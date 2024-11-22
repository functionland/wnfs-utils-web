# wnfs-utils

This library is used with wnfs-android to feed custom blockstore to wnfs-android (from Fula interface)

## Build and Use in TypeScript

1. Build your WebAssembly module:

```
wasm-pack build --target web
```

2. edit package name in pkg/package.json. Install the generated package in your Next.js project:

```
npm install ../path/to/your/pkg
```

3. Use in your React components:

```
import { WasmPrivateDirectoryHelper } from 'wnfsutils';
import { FFIStore } from 'wnfs-wasm';

function MyComponent() {
  useEffect(() => {
    async function init() {
      // Create FFIStore implementation using wnfs-wasm
      const store = new FFIStore();
      
      // Initialize helper
      const helper = new WasmPrivateDirectoryHelper(store);
      
      // Use methods
      try {
        const content = new TextEncoder().encode("Hello WNFS!");
        await helper.write_file("/test.txt", content, Date.now());
        
        const readContent = await helper.read_file("/test.txt");
        console.log(new TextDecoder().decode(readContent));
      } catch (err) {
        console.error(err);
      }
    }
    
    init();
  }, []);

  return <div>My WNFS Component</div>;
}
```
