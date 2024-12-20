//! This example shows how to add a directory to a private forest (also HAMT) which encrypts it.
//! It also shows how to retrieve encrypted nodes from the forest using `AccessKey`s.

use async_trait::async_trait;
use chrono::prelude::*;
use libipld::Cid;
use rand::{rngs::ThreadRng, thread_rng};
use rand_chacha::ChaCha12Rng;
use rand_core::SeedableRng;
use rsa::{traits::PublicKeyParts, BigUint, Oaep, RsaPrivateKey, RsaPublicKey};
use std::{rc::Rc, sync::Mutex};

use wnfs::{
    common::{BlockStore, Metadata, CODEC_RAW},
    nameaccumulator::AccumulatorSetup,
    private::{
        forest::{hamt::HamtForest, traits::PrivateForest},
        share::{recipient, sharer},
        AccessKey, ExchangeKey, PrivateDirectory, PrivateKey, PUBLIC_KEY_EXPONENT,
    },
    public::{PublicDirectory, PublicLink, PublicNode},
};

use anyhow::{anyhow, Result};
use log::trace;
use sha3::Sha3_256;

use crate::blockstore::FFIFriendlyBlockStore;

#[derive(Clone)]
struct State {
    initialized: bool,
    wnfs_key: Vec<u8>,
}
impl State {
    fn update(&mut self, initialized: bool, wnfs_key: Vec<u8>) {
        self.initialized = initialized;
        self.wnfs_key = wnfs_key;
    }
}
static mut STATE: Mutex<State> = Mutex::new(State {
    initialized: false,
    wnfs_key: Vec::new(),
});

pub struct PrivateDirectoryHelper<'a> {
    pub store: FFIFriendlyBlockStore<'a>,
    forest: Rc<HamtForest>,
    root_dir: Rc<PrivateDirectory>,
    rng: ThreadRng,
}

// Single root (private ref) implementation of the wnfs private directory using KVBlockStore.
// TODO: we assumed all the write, mkdirs use same roots here. this could be done using prepend
// a root path to all path segments.
impl<'a> PrivateDirectoryHelper<'a> {
    // Public getter for the forest field
    pub fn forest(&self) -> &Rc<HamtForest> {
        &self.forest
    }

    // Public getter for the root_dir field (if needed)
    pub fn root_dir(&self) -> &Rc<PrivateDirectory> {
        &self.root_dir
    }
    async fn reload(
        store: &mut FFIFriendlyBlockStore<'a>,
        cid: Cid,
    ) -> Result<PrivateDirectoryHelper<'a>, String> {
        let initialized: bool;
        let wnfs_key: Vec<u8>;
        unsafe {
            initialized = STATE.lock().unwrap().initialized;
            wnfs_key = STATE.lock().unwrap().wnfs_key.to_owned();
        }
        if initialized {
            let helper_res =
                PrivateDirectoryHelper::load_with_wnfs_key(store, cid, wnfs_key.to_owned()).await;
            if helper_res.is_ok() {
                Ok(helper_res.ok().unwrap())
            } else {
                trace!(
                    "wnfsError in new: {:?}",
                    helper_res.as_ref().err().unwrap().to_string()
                );
                Err(helper_res.err().unwrap().to_string())
            }
        } else {
            Err("PrivateDirectoryHelper not initialized".into())
        }
    }

    fn bytes_to_hex_str(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{:02x}", byte)).collect()
    }

    async fn setup_seeded_keypair_access(
        forest: &mut Rc<HamtForest>,
        access_key: AccessKey,
        store: &mut FFIFriendlyBlockStore<'a>,
        seed: [u8; 32],
    ) -> Result<[u8; 32]> {
        let root_did = Self::bytes_to_hex_str(&seed);
        let exchange_keypair = SeededExchangeKey::from_seed(seed.clone())?;

        // Store the public key inside some public WNFS.
        // Building from scratch in this case. Would actually be stored next to the private forest usually.
        let public_key_cid = exchange_keypair.store_public_key(store).await?;
        let mut exchange_root = Rc::new(PublicDirectory::new(Utc::now()));
        exchange_root
            .write(
                &["main".into(), "v1.exchange_key".into()],
                public_key_cid,
                Utc::now(),
                store,
            )
            .await?;
        let exchange_root = PublicLink::new(PublicNode::Dir(exchange_root));

        // The user identity's root DID. In practice this would be e.g. an ed25519 key used
        // for e.g. UCANs or key usually used for authenticating writes.

        let counter = recipient::find_latest_share_counter(
            0,
            1000,
            &exchange_keypair.encode_public_key(),
            &root_did,
            forest,
            store,
        )
        .await?
        .map(|x| x + 1)
        .unwrap_or_default();

        // Write the encrypted AccessKey into the forest
        sharer::share::<PublicExchangeKey>(
            &access_key,
            counter,
            &root_did,
            exchange_root,
            forest,
            store,
        )
        .await?;
        Ok(seed)
    }

    async fn init(
        store: &mut FFIFriendlyBlockStore<'a>,
        wnfs_key: Vec<u8>,
    ) -> Result<(PrivateDirectoryHelper<'a>, AccessKey, Cid), String> {
        let rng = &mut thread_rng();
        if wnfs_key.is_empty() {
            let err = "wnfskey is empty".to_string();
            trace!("wnfsError occured in init: {:?}", err);
            return Err(err);
        }

        let forest_res = PrivateDirectoryHelper::create_private_forest(store.to_owned(), rng).await;

        if forest_res.is_ok() {
            let (forest, _) = &mut forest_res.ok().unwrap();
            let root_dir_res = PrivateDirectory::new_and_store(
                &forest.empty_name(),
                Utc::now(),
                forest,
                store,
                rng,
            )
            .await;

            if root_dir_res.is_ok() {
                // Private ref contains data and keys for fetching and decrypting the directory node in the private forest.
                let root_dir = &mut root_dir_res.ok().unwrap();
                let access_key = root_dir.as_node().store(forest, store, rng).await;
                if access_key.is_ok() {
                    let seed: [u8; 32] = wnfs_key.to_owned().try_into().expect("Length mismatch");
                    let access_key_unwrapped = access_key.ok().unwrap();
                    let seed_res = Self::setup_seeded_keypair_access(
                        forest,
                        access_key_unwrapped.to_owned(),
                        store,
                        seed,
                    )
                    .await;
                    let forest_cid = PrivateDirectoryHelper::update_private_forest(
                        store.to_owned(),
                        forest.to_owned(),
                    )
                    .await;
                    if forest_cid.is_ok() {
                        unsafe {
                            STATE.lock().unwrap().update(true, wnfs_key.to_owned());
                        }
                        Ok((
                            Self {
                                store: store.to_owned(),
                                forest: forest.to_owned(),
                                root_dir: root_dir.to_owned(),
                                rng: rng.to_owned(),
                            },
                            access_key_unwrapped,
                            forest_cid.unwrap(),
                        ))
                    } else {
                        trace!(
                            "wnfsError in init:setup_seeded_keypair_access : {:?}",
                            seed_res.as_ref().err().unwrap().to_string()
                        );
                        Err(seed_res.err().unwrap().to_string())
                    }
                } else {
                    trace!(
                        "wnfsError in init: {:?}",
                        access_key.as_ref().err().unwrap().to_string()
                    );
                    Err(access_key.err().unwrap().to_string())
                }
            } else {
                trace!(
                    "wnfsError occured in init: {:?}",
                    root_dir_res.as_ref().to_owned().err().unwrap().to_string()
                );
                Err(root_dir_res.as_ref().to_owned().err().unwrap().to_string())
            }
        } else {
            let err = forest_res.as_ref().to_owned().err().unwrap().to_string();
            trace!("wnfsError occured in init: {:?}", err);
            Err(err)
        }
    }

    pub async fn load_with_wnfs_key(
        store: &mut FFIFriendlyBlockStore<'a>,
        forest_cid: Cid,
        wnfs_key: Vec<u8>,
    ) -> Result<PrivateDirectoryHelper<'a>, String> {
        trace!("wnfsutils: load_with_wnfs_key started");
        let rng = &mut thread_rng();
        let root_did: String;
        let seed: [u8; 32];
        if wnfs_key.is_empty() {
            let err = "wnfskey is empty".to_string();
            trace!("wnfsError occured in load_with_wnfs_key: {:?}", err);
            return Err(err);
        } else {
            root_did = Self::bytes_to_hex_str(&wnfs_key);
            seed = wnfs_key.to_owned().try_into().expect("Length mismatch");
        }
        let exchange_keypair_res = SeededExchangeKey::from_seed(seed);
        if exchange_keypair_res.is_ok() {
            let exchange_keypair = exchange_keypair_res.ok().unwrap();
            trace!(
                "wnfsutils: load_with_wnfs_key with forest_cid: {:?}",
                forest_cid
            );
            let forest_res =
                PrivateDirectoryHelper::load_private_forest(store.to_owned(), forest_cid).await;
            if forest_res.is_ok() {
                let forest = &mut forest_res.ok().unwrap();
                // Re-load private node from forest
                let counter_res = recipient::find_latest_share_counter(
                    0,
                    1000,
                    &exchange_keypair.encode_public_key(),
                    &root_did,
                    forest,
                    store,
                )
                .await;
                if counter_res.is_ok() {
                    let counter = counter_res.ok().unwrap().map(|x| x).unwrap_or_default();
                    trace!("wnfsutils: load_with_wnfs_key with counter: {:?}", counter);
                    let name = sharer::create_share_name(
                        counter,
                        &root_did,
                        &exchange_keypair.encode_public_key(),
                        forest,
                    );
                    let node_res =
                        recipient::receive_share(&name, &exchange_keypair, forest, store).await;
                    if node_res.is_ok() {
                        let node = node_res.ok().unwrap();
                        let latest_node = node.search_latest(forest, store).await;

                        if latest_node.is_ok() {
                            let latest_root_dir = latest_node.ok().unwrap().as_dir();
                            if latest_root_dir.is_ok() {
                                unsafe {
                                    STATE.lock().unwrap().update(true, wnfs_key.to_owned());
                                }
                                Ok(Self {
                                    store: store.to_owned(),
                                    forest: forest.to_owned(),
                                    root_dir: latest_root_dir.ok().unwrap(),
                                    rng: rng.to_owned(),
                                })
                            } else {
                                trace!(
                                    "wnfsError in load_with_wnfs_key: {:?}",
                                    latest_root_dir.as_ref().err().unwrap().to_string()
                                );
                                Err(latest_root_dir.err().unwrap().to_string())
                            }
                        } else {
                            trace!(
                                "wnfsError occured in load_with_wnfs_key: {:?}",
                                latest_node.as_ref().to_owned().err().unwrap().to_string()
                            );
                            Err(latest_node.as_ref().to_owned().err().unwrap().to_string())
                        }
                    } else {
                        let err = node_res.as_ref().to_owned().err().unwrap().to_string();
                        trace!(
                            "wnfsError occured in load_with_wnfs_key node_res: {:?}",
                            err
                        );
                        Err(err)
                    }
                } else {
                    let err = counter_res.as_ref().to_owned().err().unwrap().to_string();
                    trace!(
                        "wnfsError occured in load_with_wnfs_key counter_res: {:?}",
                        err
                    );
                    Err(err)
                }
            } else {
                let err = forest_res.as_ref().to_owned().err().unwrap().to_string();
                trace!("wnfsError occured in load_with_wnfs_key: {:?}", err);
                Err(err)
            }
        } else {
            let err = exchange_keypair_res
                .as_ref()
                .to_owned()
                .err()
                .unwrap()
                .to_string();
            trace!(
                "wnfsError occured in load_with_wnfs_key exchange_keypair_res: {:?}",
                err
            );
            Err(err)
        }
    }

    async fn create_private_forest(
        store: FFIFriendlyBlockStore<'a>,
        rng: &mut ThreadRng,
    ) -> Result<(Rc<HamtForest>, Cid), String> {
        // Do a trusted setup for WNFS' name accumulators
        let setup = AccumulatorSetup::trusted(rng);

        // Create the private forest (a HAMT), a map-like structure where file and directory ciphertexts are stored.
        let forest = &mut HamtForest::new_rc(setup);

        // Doing this will give us a single root CID
        let private_root_cid = store.put_async_serializable(forest).await;
        if private_root_cid.is_ok() {
            let ret_forest = Rc::clone(forest);
            Ok((ret_forest, private_root_cid.ok().unwrap()))
        } else {
            trace!(
                "wnfsError occured in create_private_forest: {:?}",
                private_root_cid.as_ref().err().unwrap()
            );
            Err(private_root_cid.err().unwrap().to_string())
        }
    }

    async fn load_private_forest(
        store: FFIFriendlyBlockStore<'a>,
        forest_cid: Cid,
    ) -> Result<Rc<HamtForest>, String> {
        // Deserialize private forest from the blockstore.
        let forest = store.get_deserializable::<HamtForest>(&forest_cid).await;
        if forest.is_ok() {
            Ok(Rc::new(forest.ok().unwrap()))
        } else {
            trace!(
                "wnfsError occured in load__private_forest: {:?}",
                forest.as_ref().err().unwrap()
            );
            Err(forest.err().unwrap().to_string())
        }
    }

    pub async fn update_private_forest(
        store: FFIFriendlyBlockStore<'a>,
        forest: Rc<HamtForest>,
    ) -> Result<Cid, String> {
        // Serialize the private forest to DAG CBOR.
        // Doing this will give us a single root CID
        let private_root_cid = store.put_async_serializable(&forest).await;
        if private_root_cid.is_ok() {
            Ok(private_root_cid.ok().unwrap())
        } else {
            trace!(
                "wnfsError occured in create_private_forest: {:?}",
                private_root_cid.as_ref().err().unwrap()
            );
            Err(private_root_cid.err().unwrap().to_string())
        }
    }

    pub async fn write_file(
        &mut self,

        path_segments: &[String],
        content: Vec<u8>,
        modification_time_seconds: i64,
    ) -> Result<Cid, String> {
        let forest = &mut self.forest;
        let root_dir = &mut self.root_dir;
        let mut modification_time_utc: DateTime<Utc> = Utc::now();
        if modification_time_seconds > 0 {
            let naive_datetime = DateTime::from_timestamp(modification_time_seconds, 0)
                .unwrap()
                .naive_utc();
            modification_time_utc = DateTime::from_naive_utc_and_offset(naive_datetime, Utc);
        }
        let write_res = root_dir
            .write(
                path_segments,
                true,
                modification_time_utc,
                content,
                forest,
                &mut self.store,
                &mut self.rng,
            )
            .await;
        if write_res.is_ok() {
            // Private ref contains data and keys for fetching and decrypting the directory node in the private forest.
            let access_key = root_dir
                .as_node()
                .store(forest, &mut self.store, &mut self.rng)
                .await;
            if access_key.is_ok() {
                let forest_cid = PrivateDirectoryHelper::update_private_forest(
                    self.store.to_owned(),
                    forest.to_owned(),
                )
                .await;
                if forest_cid.is_ok() {
                    Ok(forest_cid.ok().unwrap())
                } else {
                    trace!(
                        "wnfsError in write_file: {:?}",
                        forest_cid.as_ref().err().unwrap().to_string()
                    );
                    Err(forest_cid.err().unwrap().to_string())
                }
            } else {
                trace!(
                    "wnfsError in write_file: {:?}",
                    access_key.as_ref().err().unwrap().to_string()
                );
                Err(access_key.err().unwrap().to_string())
            }
        } else {
            trace!(
                "wnfsError in write_file: {:?}",
                write_res.as_ref().err().unwrap().to_string()
            );
            Err(write_res.err().unwrap().to_string())
        }
    }

    pub async fn read_file(&mut self, path_segments: &[String]) -> Result<Vec<u8>, String> {
        let forest = &mut self.forest;
        let root_dir = &mut self.root_dir;
        let res = root_dir
            .read(path_segments, true, forest, &mut self.store)
            .await;
        if res.is_ok() {
            let result = res.ok().unwrap();
            Ok(result)
        } else {
            trace!(
                "wnfsError occured in read_file: {:?} ",
                res.as_ref().err().unwrap()
            );
            Err(res.err().unwrap().to_string())
        }
    }

    pub async fn mkdir(&mut self, path_segments: &[String]) -> Result<Cid, String> {
        let forest = &mut self.forest;
        let root_dir = &mut self.root_dir;
        let res = root_dir
            .mkdir(
                path_segments,
                true,
                Utc::now(),
                forest,
                &mut self.store,
                &mut self.rng,
            )
            .await;
        if res.is_ok() {
            // Private ref contains data and keys for fetching and decrypting the directory node in the private forest.
            let access_key = root_dir
                .as_node()
                .store(forest, &mut self.store, &mut self.rng)
                .await;
            if access_key.is_ok() {
                let forest_cid = PrivateDirectoryHelper::update_private_forest(
                    self.store.to_owned(),
                    forest.to_owned(),
                )
                .await;
                if forest_cid.is_ok() {
                    Ok(forest_cid.ok().unwrap())
                } else {
                    trace!(
                        "wnfsError in mkdir: {:?}",
                        forest_cid.as_ref().err().unwrap().to_string()
                    );
                    Err(forest_cid.err().unwrap().to_string())
                }
            } else {
                trace!(
                    "wnfsError in mkdir: {:?}",
                    access_key.as_ref().err().unwrap().to_string()
                );
                Err(access_key.err().unwrap().to_string())
            }
        } else {
            trace!(
                "wnfsError occured in mkdir: {:?}",
                res.as_ref().err().unwrap()
            );
            Err(res.err().unwrap().to_string())
        }
    }

    pub async fn rm(&mut self, path_segments: &[String]) -> Result<Cid, String> {
        let forest = &mut self.forest;
        let root_dir = &mut self.root_dir;
        let result = root_dir
            .rm(path_segments, true, forest, &mut self.store)
            .await;
        if result.is_ok() {
            // Private ref contains data and keys for fetching and decrypting the directory node in the private forest.
            let access_key = root_dir
                .as_node()
                .store(forest, &mut self.store, &mut self.rng)
                .await;
            if access_key.is_ok() {
                let forest_cid = PrivateDirectoryHelper::update_private_forest(
                    self.store.to_owned(),
                    forest.to_owned(),
                )
                .await;
                if forest_cid.is_ok() {
                    Ok(forest_cid.ok().unwrap())
                } else {
                    trace!(
                        "wnfsError in result: {:?}",
                        forest_cid.as_ref().err().unwrap().to_string()
                    );
                    Err(forest_cid.err().unwrap().to_string())
                }
            } else {
                trace!(
                    "wnfsError in result: {:?}",
                    access_key.as_ref().err().unwrap().to_string()
                );
                Err(access_key.err().unwrap().to_string())
            }
        } else {
            trace!(
                "wnfsError occured in rm result: {:?}",
                result.as_ref().err().unwrap()
            );
            Err(result.err().unwrap().to_string())
        }
    }

    pub async fn mv(
        &mut self,
        source_path_segments: &[String],
        target_path_segments: &[String],
    ) -> Result<Cid, String> {
        let forest = &mut self.forest;
        let root_dir = &mut self.root_dir;
        let mv_result = root_dir
            .basic_mv(
                source_path_segments,
                target_path_segments,
                true,
                Utc::now(),
                forest,
                &mut self.store,
                &mut self.rng,
            )
            .await;
        if mv_result.is_ok() {
            // Private ref contains data and keys for fetching and decrypting the directory node in the private forest.
            let access_key = root_dir
                .as_node()
                .store(forest, &mut self.store, &mut self.rng)
                .await;
            if access_key.is_ok() {
                let forest_cid = PrivateDirectoryHelper::update_private_forest(
                    self.store.to_owned(),
                    forest.to_owned(),
                )
                .await;
                if forest_cid.is_ok() {
                    Ok(forest_cid.ok().unwrap())
                } else {
                    trace!(
                        "wnfsError in mv_result: {:?}",
                        forest_cid.as_ref().err().unwrap().to_string()
                    );
                    Err(forest_cid.err().unwrap().to_string())
                }
            } else {
                trace!(
                    "wnfsError in mv_result: {:?}",
                    access_key.as_ref().err().unwrap().to_string()
                );
                Err(access_key.err().unwrap().to_string())
            }
        } else {
            trace!(
                "wnfsError occured in mv mv_result: {:?}",
                mv_result.as_ref().err().unwrap()
            );
            Err(mv_result.err().unwrap().to_string())
        }
    }

    pub async fn cp(
        &mut self,
        source_path_segments: &[String],
        target_path_segments: &[String],
    ) -> Result<Cid, String> {
        let forest = &mut self.forest;
        let root_dir = &mut self.root_dir;
        let cp_result = root_dir
            .cp(
                source_path_segments,
                target_path_segments,
                true,
                Utc::now(),
                forest,
                &mut self.store,
                &mut self.rng,
            )
            .await;
        if cp_result.is_ok() {
            // Private ref contains data and keys for fetching and decrypting the directory node in the private forest.
            let access_key = root_dir
                .as_node()
                .store(forest, &mut self.store, &mut self.rng)
                .await;
            if access_key.is_ok() {
                let forest_cid = PrivateDirectoryHelper::update_private_forest(
                    self.store.to_owned(),
                    forest.to_owned(),
                )
                .await;
                if forest_cid.is_ok() {
                    Ok(forest_cid.ok().unwrap())
                } else {
                    trace!(
                        "wnfsError in cp_result: {:?}",
                        forest_cid.as_ref().err().unwrap().to_string()
                    );
                    Err(forest_cid.err().unwrap().to_string())
                }
            } else {
                trace!(
                    "wnfsError in cp_result: {:?}",
                    access_key.as_ref().err().unwrap().to_string()
                );
                Err(access_key.err().unwrap().to_string())
            }
        } else {
            trace!(
                "wnfsError occured in cp cp_result: {:?}",
                cp_result.as_ref().err().unwrap()
            );
            Err(cp_result.err().unwrap().to_string())
        }
    }

    pub async fn ls_files(
        &mut self,
        path_segments: &[String],
    ) -> Result<Vec<(String, Metadata)>, String> {
        let forest = &mut self.forest;
        let root_dir = &mut self.root_dir;
        let res = root_dir
            .ls(path_segments, true, forest, &mut self.store)
            .await;
        if res.is_ok() {
            let result = res.ok().unwrap();
            Ok(result)
        } else {
            trace!(
                "wnfsError occured in ls_files: {:?}",
                res.as_ref().err().unwrap().to_string()
            );
            Err(res.err().unwrap().to_string())
        }
    }
}

// Implement synced version of the library for using in android jni.
impl<'a> PrivateDirectoryHelper<'a> {
    pub async fn init_async(
        store: &mut FFIFriendlyBlockStore<'a>,
        wnfs_key: Vec<u8>,
    ) -> Result<(PrivateDirectoryHelper<'a>, AccessKey, Cid), String> {
        PrivateDirectoryHelper::init(store, wnfs_key).await
    }

    pub async fn load_with_wnfs_key_async(
        store: &mut FFIFriendlyBlockStore<'a>,
        forest_cid: Cid,
        wnfs_key: Vec<u8>,
    ) -> Result<PrivateDirectoryHelper<'a>, String> {
        PrivateDirectoryHelper::load_with_wnfs_key(store, forest_cid, wnfs_key).await
    }

    pub async fn reload_async(
        store: &mut FFIFriendlyBlockStore<'a>,
        forest_cid: Cid,
    ) -> Result<PrivateDirectoryHelper<'a>, String> {
        PrivateDirectoryHelper::reload(store, forest_cid).await
    }

    pub async fn write_file_async(
        &mut self,
        path_segments: &[String],
        content: Vec<u8>,
        modification_time_seconds: i64,
    ) -> Result<Cid, String> {
        self.write_file(path_segments, content, modification_time_seconds).await
    }
    
    pub async fn read_file_async(&mut self, path_segments: &[String]) -> Result<Vec<u8>, String> {
        self.read_file(path_segments).await
    }
    
    pub async fn mkdir_async(&mut self, path_segments: &[String]) -> Result<Cid, String> {
        self.mkdir(path_segments).await
    }
    
    pub async fn mv_async(
        &mut self,
        source_path_segments: &[String],
        target_path_segments: &[String],
    ) -> Result<Cid, String> {
        self.mv(source_path_segments, target_path_segments).await
    }
    
    pub async fn cp_async(
        &mut self,
        source_path_segments: &[String],
        target_path_segments: &[String],
    ) -> Result<Cid, String> {
        self.cp(source_path_segments, target_path_segments).await
    }
    
    pub async fn rm_async(&mut self, path_segments: &[String]) -> Result<Cid, String> {
        self.rm(path_segments).await
    }
    
    pub async fn ls_files_async(
        &mut self,
        path_segments: &[String],
    ) -> Result<Vec<(String, Metadata)>, String> {
        self.ls_files(path_segments).await
    }

    pub fn parse_path(path: String) -> Vec<String> {
        path.trim()
            .trim_matches('/')
            .split("/")
            .map(|s| s.to_string())
            .collect()
    }
}

struct SeededExchangeKey(RsaPrivateKey);

struct PublicExchangeKey(RsaPublicKey);

impl SeededExchangeKey {
    pub fn from_seed(seed: [u8; 32]) -> Result<Self> {
        let rng = &mut ChaCha12Rng::from_seed(seed);
        let private_key = RsaPrivateKey::new(rng, 2048)?;
        Ok(Self(private_key))
    }

    pub async fn store_public_key(&self, store: &impl BlockStore) -> Result<Cid> {
        store.put_block(self.encode_public_key(), CODEC_RAW).await
    }

    pub fn encode_public_key(&self) -> Vec<u8> {
        self.0.n().to_bytes_be()
    }
}

#[async_trait(?Send)]
impl PrivateKey for SeededExchangeKey {
    async fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        let padding = Oaep::new::<Sha3_256>();
        self.0.decrypt(padding, ciphertext).map_err(|e| anyhow!(e))
    }
}

#[async_trait(?Send)]
impl ExchangeKey for PublicExchangeKey {
    async fn encrypt(&self, data: &[u8]) -> Result<Vec<u8>> {
        let padding = Oaep::new::<Sha3_256>();
        self.0
            .encrypt(&mut rand::thread_rng(), padding, data)
            .map_err(|e| anyhow!(e))
    }

    async fn from_modulus(modulus: &[u8]) -> Result<Self> {
        let n = BigUint::from_bytes_be(modulus);
        let e = BigUint::from(PUBLIC_KEY_EXPONENT);

        Ok(Self(rsa::RsaPublicKey::new(n, e).map_err(|e| anyhow!(e))?))
    }
}

#[cfg(test)]
mod private_forest_tests;
