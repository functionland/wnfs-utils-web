use anyhow::Result;
use async_trait::async_trait;
use bytes::Bytes;
use libipld::Cid;
use wnfs::common::{BlockStore, BlockStoreError};

pub trait FFIStore: FFIStoreClone {
    fn get_block(&self, cid: Vec<u8>) -> Result<Vec<u8>>;
    fn put_block(&self, cid: Vec<u8>, bytes: Vec<u8>) -> Result<()>;
}

pub trait FFIStoreClone {
    fn clone_box(&self) -> Box<dyn FFIStore>;
}

impl<T> FFIStoreClone for T
where
    T: FFIStore + Clone + 'static,
{
    fn clone_box(&self) -> Box<dyn FFIStore> {
        Box::new(self.clone())
    }
}

impl Clone for Box<dyn FFIStore> {
    fn clone(&self) -> Box<dyn FFIStore> {
        self.clone_box()
    }
}

#[derive(Clone)]
pub struct FFIFriendlyBlockStore {
    pub ffi_store: Box<dyn FFIStore>,
}

impl FFIFriendlyBlockStore {
    pub fn new(ffi_store: Box<dyn FFIStore>) -> Self {
        Self { ffi_store }
    }
}

#[async_trait(?Send)]
impl BlockStore for FFIFriendlyBlockStore {
    async fn get_block(&self, cid: &Cid) -> Result<Bytes> {
        let bytes = self
            .ffi_store
            .get_block(cid.to_bytes())
            .map_err(|_| BlockStoreError::CIDNotFound(*cid))?;
        Ok(Bytes::copy_from_slice(&bytes))
    }

    async fn put_block(&self, bytes: impl Into<Bytes>, codec: u64) -> Result<Cid> {
        let data: Bytes = bytes.into();

        let cid_res = self.create_cid(&data, codec);
        match cid_res.is_err() {
            true => Err(cid_res.err().unwrap()),
            false => {
                let cid = cid_res.unwrap();
                let result = self
                    .ffi_store
                    .put_block(cid.to_owned().to_bytes(), data.to_vec());
                match result {
                    Ok(_) => Ok(cid.to_owned()),
                    Err(e) => Err(e),
                }
            }
        }
    }
}

#[cfg(test)]
mod blockstore_tests;