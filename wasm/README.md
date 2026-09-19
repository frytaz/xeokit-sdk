# WebAssembly loaders

Rust crates behind `GLTFLoaderPlugin` and `LASLoaderPlugin`, compiled to `wasm32-unknown-unknown` and embedded in
the SDK as base64 strings (`src/plugins/GLTFLoaderPlugin/gltfWasm.js`, `src/plugins/LASLoaderPlugin/lasWasm.js`),
so the single-file `dist/` bundles keep working without any extra assets to host.

| Crate  | Module                         | Scope                                                                                          |
|--------|--------------------------------|------------------------------------------------------------------------------------------------|
| `las`  | `xeokit_las.wasm` (~130 KB)    | LAS 1.0 - 1.4 (R15) headers and point records, LASzip decompression (compressors 1, 2, 3; item versions 1 - 4) |
| `gltf` | `xeokit_gltf.wasm` (~170 KB)   | glTF 1.0 and 2.x containers (JSON, GLB v1/v2), glTF 1.0 normalization, accessors, KHR_texture_transform, embedded images |

Both expose a plain C ABI (no wasm-bindgen): `alloc`/`dealloc` for passing bytes, `error_ptr`/`error_len` for the
last error, and the loader-specific calls documented in each crate's `src/lib.rs`. The JavaScript side
(`src/plugins/lib/wasm/loadWasm.js`, `parseLAS.js`, `parseGLTF.js`) only copies bytes in and out, fetches external
resources, decodes images and links object references.

## Building

Requires Rust (edition 2021) with the wasm32 target: `rustup target add wasm32-unknown-unknown`.
[binaryen](https://github.com/WebAssembly/binaryen)'s `wasm-opt` is used automatically when it is on the PATH.

```
npm run build:wasm   # cargo build --release --target wasm32-unknown-unknown, then embeds the binaries in src/
npm run test:wasm    # native unit tests (byte-exact LAZ decoding against reference hashes, glTF samples)
```

The generated `*Wasm.js` modules are committed, so building the SDK itself (`npm run build`) does not need Rust.
Run `npm run build:wasm` after changing anything under `wasm/`.

## Test data

`las/tests/data/simple-laszip-1.2r0.laz` comes from the [PDAL](https://github.com/PDAL/PDAL) test suite (BSD) and
`las/tests/data/autzen.copc.laz` from [las-rs](https://github.com/gadomski/las-rs) (MIT/Apache-2.0). The other tests
use models from `assets/models`.
