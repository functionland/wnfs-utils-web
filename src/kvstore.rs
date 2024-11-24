use kv::*;
use anyhow::Result;
use libipld::Cid;
use wnfs::common::BlockStoreError;
use crate::blockstore::FFIStore;
use async_trait::async_trait;

#[derive(Clone)]
pub struct KVBlockStore {
    pub store: Store,
    pub codec: u64,
}

//--------------------------------------------------------------------------------------------------
// Implementations
//--------------------------------------------------------------------------------------------------

impl KVBlockStore {
    /// Creates a new kv block store.
    pub fn new(db_path: String, codec: u64) -> Self {
        // Configure the database
        // Open the key/value store
        Self {
            store: Store::new(Config::new(db_path)).unwrap(),
            codec,
        }
    }
}

#[async_trait(?Send)]
impl<'a> FFIStore<'a> for KVBlockStore {
    /// Retrieves an array of bytes from the block store with given CID.
    async fn get_block(&self, cid: Vec<u8>) -> Result<Vec<u8>> {
        // Offload the blocking operation to a separate thread
        let store = self.store.clone();
        let cid_clone = cid.clone(); // Clone cid for use in the closure

        let result = tokio::task::spawn_blocking(move || {
            // Perform the blocking operation
            let bucket = store.bucket::<Raw, Raw>(Some("default"))?;
            let bytes = bucket
                .get(&Raw::from(cid_clone.clone())) // Clone cid_clone here
                .map_err(|_| BlockStoreError::CIDNotFound(Cid::try_from(cid_clone.clone()).unwrap()))?
                .ok_or_else(|| BlockStoreError::CIDNotFound(Cid::try_from(cid_clone.clone()).unwrap()))?
                .to_vec();
            Ok::<Vec<u8>, anyhow::Error>(bytes)
        })
        .await;

        // Handle errors from spawn_blocking and return the result
        result.map_err(|e| anyhow::Error::msg(format!("Failed to retrieve block: {:?}", e)))?
    }

    /// Stores an array of bytes in the block store.
    async fn put_block(&self, cid: Vec<u8>, bytes: Vec<u8>) -> Result<()> {
        // Offload the blocking operation to a separate thread
        let store = self.store.clone();
        let cid_clone = cid.clone(); // Clone cid for use in the closure
        let bytes_clone = bytes.clone(); // Clone bytes for use in the closure

        let result = tokio::task::spawn_blocking(move || {
            // Perform the blocking operation
            let bucket = store.bucket::<Raw, Raw>(Some("default"))?;
            let key = Raw::from(cid_clone);
            let value = Raw::from(bytes_clone);

            bucket.set(&key, &value)?;
            Ok::<(), anyhow::Error>(())
        })
        .await;

        // Handle errors from spawn_blocking and return the result
        result.map_err(|e| anyhow::Error::msg(format!("Failed to store block: {:?}", e)))?
    }
}