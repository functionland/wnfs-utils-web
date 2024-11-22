pub mod blockstore;
pub mod kvstore;
pub mod private_forest;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct WasmPrivateDirectoryHelper {
    inner: PrivateDirectoryHelper<'static>
}

#[wasm_bindgen]
impl WasmPrivateDirectoryHelper {
    #[wasm_bindgen(constructor)]
    pub fn new(store: Box<dyn FFIStore<'static>>) -> Result<WasmPrivateDirectoryHelper, JsValue> {
        let blockstore = FFIFriendlyBlockStore::new(store);
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

