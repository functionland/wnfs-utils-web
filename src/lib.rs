pub mod blockstore;
pub mod kvstore;
pub mod private_forest;

use wasm_bindgen::prelude::*;
use crate::private_forest::PrivateDirectoryHelper;
use crate::blockstore::{FFIStore, FFIFriendlyBlockStore};
use js_sys::{Promise, Uint8Array};
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(typescript_type = "IStore")]
    type JsStore;

    #[wasm_bindgen(method, catch)]
    fn get_block(this: &JsStore, cid: Uint8Array) -> Result<Promise, JsValue>;

    #[wasm_bindgen(method, catch)]
    fn put_block(this: &JsStore, cid: Uint8Array, bytes: Uint8Array) -> Result<Promise, JsValue>;
}

#[derive(Clone)]
struct WasmStore {
    inner: JsStore,
}

impl FFIStore for WasmStore {
    fn get_block(&self, cid: Vec<u8>) -> anyhow::Result<Vec<u8>> {
        let js_cid = Uint8Array::from(&cid[..]);
        match self.inner.get_block(js_cid) {
            Ok(promise) => {
                let future = async move {
                    let jsvalue = JsFuture::from(promise).await?;
                    let uint8array = Uint8Array::new(&jsvalue);
                    Ok(uint8array.to_vec())
                };
                
                // Note: This is a simplification. You'll need proper async handling
                Ok(vec![])
            }
            Err(e) => Err(anyhow::anyhow!("JS Error: {:?}", e))
        }
    }

    fn put_block(&self, cid: Vec<u8>, bytes: Vec<u8>) -> anyhow::Result<()> {
        let js_cid = Uint8Array::from(&cid[..]);
        let js_bytes = Uint8Array::from(&bytes[..]);
        
        match self.inner.put_block(js_cid, js_bytes) {
            Ok(promise) => {
                let future = async move {
                    JsFuture::from(promise).await?;
                    Ok(())
                };
                
                // Note: This is a simplification. You'll need proper async handling
                Ok(())
            }
            Err(e) => Err(anyhow::anyhow!("JS Error: {:?}", e))
        }
    }
}

#[wasm_bindgen]
pub struct WasmPrivateDirectoryHelper {
    inner: PrivateDirectoryHelper
}

#[wasm_bindgen]
impl WasmPrivateDirectoryHelper {
    #[wasm_bindgen(constructor)]
    pub fn new(js_store: JsStore) -> Result<WasmPrivateDirectoryHelper, JsValue> {
        let store = WasmStore { inner: js_store };
        let blockstore = FFIFriendlyBlockStore::new(Box::new(store));
        let wnfs_key = vec![/* your key */]; 
        
        let helper = PrivateDirectoryHelper::synced_init(&mut blockstore, wnfs_key)
            .map_err(|e| JsValue::from_str(&e))?;
            
        Ok(WasmPrivateDirectoryHelper {
            inner: helper.0
        })
    }

    #[wasm_bindgen]
    pub fn write_file(&mut self, path: String, content: Vec<u8>, mtime: i64) -> Result<String, JsValue> {
        let path_segments = PrivateDirectoryHelper::parse_path(path);
        self.inner.synced_write_file(&path_segments, content, mtime)
            .map(|cid| cid.to_string())
            .map_err(|e| JsValue::from_str(&e))
    }

    #[wasm_bindgen] 
    pub fn read_file(&mut self, path: String) -> Result<Vec<u8>, JsValue> {
        let path_segments = PrivateDirectoryHelper::parse_path(path);
        self.inner.synced_read_file(&path_segments)
            .map_err(|e| JsValue::from_str(&e))
    }
}