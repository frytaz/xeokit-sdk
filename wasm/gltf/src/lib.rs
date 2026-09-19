//! glTF parser for the xeokit SDK: glTF 1.0 (JSON, GLB version 1, KHR_binary_glTF, KHR_materials_common) and
//! glTF 2.x (JSON, GLB version 2), compiled to WebAssembly with a plain C ABI.
//!
//! The module does the format work: container parsing, glTF 1.0 normalization, buffer resolution (GLB BIN chunk,
//! data URIs; external buffers are supplied by the caller), accessor decoding (byte stride, sparse, dequantization),
//! KHR_texture_transform baking and image byte extraction. The caller fetches external resources, decodes images and
//! links the resulting index references into an object graph.

mod accessors;
mod normalize_v1;

use accessors::Decoded;
use serde_json::Value;
use std::cell::RefCell;

const GLB_MAGIC: u32 = 0x4654_6C67;
const GLB_CHUNK_JSON: u32 = 0x4E4F_534A;
const GLB_CHUNK_BIN: u32 = 0x004E_4942;
const UNSUPPORTED_REQUIRED_EXTENSIONS: [&str; 2] = ["KHR_draco_mesh_compression", "EXT_meshopt_compression"];

fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

//----------------------------------------------------------------------------------------------------------------------
// base64 and data URIs
//----------------------------------------------------------------------------------------------------------------------

fn base64_decode(s: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for &c in s {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' | b'\t' => continue,
            _ => return Err("glTF: invalid base64 data".into()),
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

fn percent_decode(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'%' && i + 2 < s.len() {
            if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&s[i + 1..i + 3]).unwrap_or("zz"), 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(s[i]);
        i += 1;
    }
    out
}

/// Decodes a `data:` URI; returns None when `uri` is not a data URI.
fn decode_data_uri(uri: &str) -> Option<Result<Vec<u8>, String>> {
    let rest = uri.strip_prefix("data:")?;
    let comma = rest.find(',')?;
    let (meta, payload) = (&rest[..comma], &rest[comma + 1..]);
    Some(if meta.ends_with(";base64") { base64_decode(payload.as_bytes()) } else { Ok(percent_decode(payload.as_bytes())) })
}

fn sniff_mime_type(uri: Option<&str>, bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 12 {
        if bytes[0] == 0x89 && bytes[1] == 0x50 {
            return Some("image/png");
        }
        if bytes[0] == 0xFF && bytes[1] == 0xD8 {
            return Some("image/jpeg");
        }
        if &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
            return Some("image/webp");
        }
        if bytes[0] == 0xAB && &bytes[1..4] == b"KTX" {
            return Some("image/ktx2");
        }
    }
    let ext = uri?.split('?').next()?.rsplit('.').next()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        "ktx2" => Some("image/ktx2"),
        "basis" => Some("image/basis"),
        _ => None,
    }
}

//----------------------------------------------------------------------------------------------------------------------
// Document
//----------------------------------------------------------------------------------------------------------------------

enum BufferSource {
    Loaded(Vec<u8>),
    Glb,
    External,
}

/// Accessor description read by JavaScript: values are `count * components` little-endian numbers of `component_type`.
#[repr(C)]
pub struct AccessorEntry {
    pub component_type: u32,
    pub components: u32,
    pub count: u32,
    pub ptr: u32,
    pub byte_len: u32,
    pub normalized: u32,
}

/// Image bytes available in module memory (`len` is 0 for images the caller has to fetch by URI).
#[repr(C)]
pub struct ImageEntry {
    pub ptr: u32,
    pub len: u32,
}

#[repr(C)]
pub struct BuildResult {
    pub json_ptr: u32,
    pub json_len: u32,
    pub accessors_ptr: u32,
    pub accessors_count: u32,
    pub images_ptr: u32,
    pub images_count: u32,
}

pub struct Doc {
    data: Vec<u8>,
    glb_bin: Option<(usize, usize)>,
    pub glb_version: u32,
    pub json: Value,
    pub json_string: String,
    buffers: Vec<BufferSource>,
    pub decoded: Vec<Decoded>,
    pub accessor_table: Vec<AccessorEntry>,
    pub image_data: Vec<Vec<u8>>,
    pub image_table: Vec<ImageEntry>,
    pub result: BuildResult,
}

fn parse_glb(data: &[u8]) -> Result<(Value, Option<(usize, usize)>, u32), String> {
    let version = rd_u32(data, 4);
    let length = (rd_u32(data, 8) as usize).min(data.len());
    let mut json = None;
    let mut bin = None;
    match version {
        1 => {
            if length < 20 {
                return Err("glTF: GLB is truncated".into());
            }
            let content_length = rd_u32(data, 12) as usize;
            if rd_u32(data, 16) != 0 {
                return Err(format!("glTF: unsupported GLB 1.0 content format {}", rd_u32(data, 16)));
            }
            if 20 + content_length > length {
                return Err("glTF: GLB is truncated".into());
            }
            json = Some(&data[20..20 + content_length]);
            bin = Some((20 + content_length, length - 20 - content_length));
        }
        2 => {
            let mut pos = 12;
            while pos + 8 <= length {
                let chunk_length = rd_u32(data, pos) as usize;
                let chunk_type = rd_u32(data, pos + 4);
                pos += 8;
                if pos + chunk_length > length {
                    return Err("glTF: GLB chunk exceeds the file".into());
                }
                if chunk_type == GLB_CHUNK_JSON && json.is_none() {
                    json = Some(&data[pos..pos + chunk_length]);
                } else if chunk_type == GLB_CHUNK_BIN && bin.is_none() {
                    bin = Some((pos, chunk_length));
                }
                pos += chunk_length + ((4 - (chunk_length & 3)) & 3);
            }
        }
        _ => return Err(format!("glTF: unsupported GLB version {}", version)),
    }
    let json = json.ok_or("glTF: GLB has no JSON chunk")?;
    let value: Value = serde_json::from_slice(json).map_err(|e| format!("glTF: invalid JSON - {}", e))?;
    Ok((value, bin, version))
}

/// Returns the major version (1 or 2), checking `asset.version` and `asset.minVersion`.
fn check_version(json: &Value) -> Result<u32, String> {
    let asset = json.get("asset");
    let version = asset.and_then(|a| a.get("version")).and_then(|v| v.as_str()).map(|s| s.to_string()).unwrap_or_else(|| {
        if json.get("meshes").map(|m| m.is_array()).unwrap_or(false) { "2.0".into() } else { "1.0".into() }
    });
    let major: u32 = version.split('.').next().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    match major {
        1 => Ok(1),
        2 => {
            if let Some(min) = asset.and_then(|a| a.get("minVersion")).and_then(|v| v.as_str()) {
                let mut parts = min.split('.').map(|s| s.trim().parse::<u32>().unwrap_or(0));
                let (min_major, min_minor) = (parts.next().unwrap_or(2), parts.next().unwrap_or(0));
                if min_major > 2 || (min_major == 2 && min_minor > 1) {
                    return Err(format!("glTF: asset requires glTF {}, this loader supports up to 2.1", min));
                }
            }
            Ok(2)
        }
        _ => Err(format!("glTF: unsupported version {}", version)),
    }
}

impl Doc {
    /// Parses a GLB or glTF JSON file. Buffers with external URIs must be supplied with `set_buffer` before `build`.
    pub fn open(data: Vec<u8>) -> Result<Doc, String> {
        let (mut json, glb_bin, glb_version) = if data.len() >= 12 && rd_u32(&data, 0) == GLB_MAGIC {
            parse_glb(&data)?
        } else {
            let text = data.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&data);
            (serde_json::from_slice(text).map_err(|e| format!("glTF: invalid JSON - {}", e))?, None, 0)
        };
        if !json.is_object() {
            return Err("glTF: JSON root must be an object".into());
        }
        if check_version(&json)? == 1 {
            normalize_v1::normalize(&mut json)?;
        }
        if let Some(required) = json.get("extensionsRequired").and_then(|v| v.as_array()) {
            for ext in UNSUPPORTED_REQUIRED_EXTENSIONS {
                if required.iter().any(|r| r.as_str() == Some(ext)) {
                    return Err(format!("glTF: unsupported required extension {}", ext));
                }
            }
        }
        let mut buffers = Vec::new();
        if let Some(list) = json.get_mut("buffers").and_then(|v| v.as_array_mut()) {
            for (i, buffer) in list.iter_mut().enumerate() {
                let obj = buffer.as_object_mut().ok_or(format!("glTF: buffer {} is not an object", i))?;
                let uri = obj.get("uri").and_then(|v| v.as_str()).map(|s| s.to_string());
                let glb_binary = obj.remove("_glbBinary").is_some();
                let source = match uri {
                    Some(uri) => match decode_data_uri(&uri) {
                        Some(bytes) => {
                            obj.remove("uri");
                            BufferSource::Loaded(bytes?)
                        }
                        None => BufferSource::External,
                    },
                    None if glb_bin.is_some() && (i == 0 || glb_binary) => BufferSource::Glb,
                    None if glb_binary || (glb_version != 0 && i == 0) => return Err("glTF: GLB has no BIN chunk".into()),
                    None => BufferSource::Loaded(vec![0; obj.get("byteLength").and_then(|v| v.as_u64()).unwrap_or(0) as usize]),
                };
                buffers.push(source);
            }
        }
        let json_string = json.to_string();
        Ok(Doc {
            data,
            glb_bin,
            glb_version,
            json,
            json_string,
            buffers,
            decoded: Vec::new(),
            accessor_table: Vec::new(),
            image_data: Vec::new(),
            image_table: Vec::new(),
            result: BuildResult { json_ptr: 0, json_len: 0, accessors_ptr: 0, accessors_count: 0, images_ptr: 0, images_count: 0 },
        })
    }

    /// URIs of the buffers that must be supplied with `set_buffer`, by buffer index.
    pub fn external_buffers(&self) -> Vec<(usize, String)> {
        self.buffers.iter().enumerate().filter(|(_, b)| matches!(b, BufferSource::External)).map(|(i, _)| {
            (i, self.json["buffers"][i]["uri"].as_str().unwrap_or("").to_string())
        }).collect()
    }

    pub fn set_buffer(&mut self, index: usize, bytes: Vec<u8>) -> Result<(), String> {
        let slot = self.buffers.get_mut(index).ok_or(format!("glTF: no buffer {}", index))?;
        *slot = BufferSource::Loaded(bytes);
        Ok(())
    }

    /// Decodes accessors and images. Requires every external buffer to have been set.
    pub fn build(&mut self) -> Result<(), String> {
        let mut slices: Vec<&[u8]> = Vec::with_capacity(self.buffers.len());
        for (i, b) in self.buffers.iter().enumerate() {
            slices.push(match b {
                BufferSource::Loaded(v) => v.as_slice(),
                BufferSource::Glb => {
                    let (off, len) = self.glb_bin.ok_or("glTF: GLB has no BIN chunk")?;
                    &self.data[off..off + len]
                }
                BufferSource::External => return Err(format!("glTF: buffer {} was not provided", i)),
            });
        }
        let views = accessors::resolve_views(&self.json, &slices)?;
        let mut decoded = Vec::new();
        for (i, accessor) in self.json.get("accessors").and_then(|v| v.as_array()).into_iter().flatten().enumerate() {
            decoded.push(accessors::decode(&views, &slices, accessor, i)?);
        }
        // Images: bytes from bufferViews and data URIs; external URIs are left to the caller
        let mut image_data = Vec::new();
        if let Some(images) = self.json.get_mut("images").and_then(|v| v.as_array_mut()) {
            for (i, image) in images.iter_mut().enumerate() {
                let obj = image.as_object_mut().ok_or(format!("glTF: image {} is not an object", i))?;
                let uri = obj.get("uri").and_then(|v| v.as_str()).map(|s| s.to_string());
                let bytes: Vec<u8> = if let Some(bv) = obj.get("bufferView").and_then(|v| v.as_u64()) {
                    let view = views.get(bv as usize).ok_or(format!("glTF: image {} references unknown bufferView {}", i, bv))?;
                    slices[view.buffer][view.offset..view.offset + view.length].to_vec()
                } else if let Some(decoded) = uri.as_deref().and_then(decode_data_uri) {
                    obj.remove("uri");
                    decoded?
                } else {
                    Vec::new()
                };
                if !obj.contains_key("mimeType") {
                    if let Some(mime) = sniff_mime_type(uri.as_deref(), &bytes) {
                        obj.insert("mimeType".into(), Value::String(mime.into()));
                    }
                }
                image_data.push(bytes);
            }
        }
        accessors::apply_texture_transforms(&mut self.json, &mut decoded)?;
        self.decoded = decoded;
        self.image_data = image_data;
        self.json_string = self.json.to_string();
        self.accessor_table = self.decoded.iter().map(|d| AccessorEntry {
            component_type: d.component_type,
            components: d.components as u32,
            count: d.count as u32,
            ptr: d.data.as_ptr() as usize as u32,
            byte_len: d.data.len() as u32,
            normalized: d.normalized as u32,
        }).collect();
        self.image_table = self.image_data.iter().map(|d| ImageEntry { ptr: d.as_ptr() as usize as u32, len: d.len() as u32 }).collect();
        self.result = BuildResult {
            json_ptr: self.json_string.as_ptr() as usize as u32,
            json_len: self.json_string.len() as u32,
            accessors_ptr: self.accessor_table.as_ptr() as usize as u32,
            accessors_count: self.accessor_table.len() as u32,
            images_ptr: self.image_table.as_ptr() as usize as u32,
            images_count: self.image_table.len() as u32,
        };
        Ok(())
    }
}

//----------------------------------------------------------------------------------------------------------------------
// C ABI for the WebAssembly build
//----------------------------------------------------------------------------------------------------------------------

thread_local! {
    static LAST_ERROR: RefCell<String> = const { RefCell::new(String::new()) };
}

fn set_error(msg: String) {
    LAST_ERROR.with(|e| *e.borrow_mut() = msg);
}

/// Allocates `len` bytes in module memory (freed by `dealloc`, or taken over by `gltf_open`/`gltf_set_buffer`).
#[no_mangle]
pub extern "C" fn alloc(len: u32) -> *mut u8 {
    let mut v: Vec<u8> = Vec::with_capacity(len as usize);
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// # Safety
/// `ptr` must come from `alloc(len)` and not have been freed or handed over.
#[no_mangle]
pub unsafe extern "C" fn dealloc(ptr: *mut u8, len: u32) {
    drop(Vec::from_raw_parts(ptr, 0, len as usize));
}

#[no_mangle]
pub extern "C" fn error_ptr() -> *const u8 {
    LAST_ERROR.with(|e| e.borrow().as_ptr())
}

#[no_mangle]
pub extern "C" fn error_len() -> u32 {
    LAST_ERROR.with(|e| e.borrow().len() as u32)
}

/// Parses a GLB or glTF JSON file held in memory allocated with `alloc` (ownership passes to the document).
/// Returns 0 on error (see `error_ptr`/`error_len`).
///
/// # Safety
/// `ptr`/`len` must describe an allocation made by `alloc`.
#[no_mangle]
pub unsafe extern "C" fn gltf_open(ptr: *mut u8, len: u32) -> *mut Doc {
    let data = Vec::from_raw_parts(ptr, len as usize, len as usize);
    match Doc::open(data) {
        Ok(doc) => Box::into_raw(Box::new(doc)),
        Err(e) => {
            set_error(e);
            std::ptr::null_mut()
        }
    }
}

/// The normalized glTF JSON (after `gltf_open`, and again after `gltf_build` with the accessors added by it).
///
/// # Safety
/// `doc` must come from `gltf_open` and not have been closed.
#[no_mangle]
pub unsafe extern "C" fn gltf_json_ptr(doc: *mut Doc) -> *const u8 {
    let d = &*doc;
    d.json_string.as_ptr()
}

/// # Safety
/// `doc` must come from `gltf_open` and not have been closed.
#[no_mangle]
pub unsafe extern "C" fn gltf_json_len(doc: *mut Doc) -> u32 {
    let d = &*doc;
    d.json_string.len() as u32
}

/// Supplies the bytes of an external buffer (ownership of the allocation passes to the document). Returns 1, or 0 on error.
///
/// # Safety
/// `doc` must come from `gltf_open`; `ptr`/`len` must describe an allocation made by `alloc`.
#[no_mangle]
pub unsafe extern "C" fn gltf_set_buffer(doc: *mut Doc, index: u32, ptr: *mut u8, len: u32) -> u32 {
    let bytes = Vec::from_raw_parts(ptr, len as usize, len as usize);
    let d = &mut *doc;
    match d.set_buffer(index as usize, bytes) {
        Ok(()) => 1,
        Err(e) => {
            set_error(e);
            0
        }
    }
}

/// Decodes accessors and images. Returns 1, or 0 on error.
///
/// # Safety
/// `doc` must come from `gltf_open` and not have been closed.
#[no_mangle]
pub unsafe extern "C" fn gltf_build(doc: *mut Doc) -> u32 {
    let d = &mut *doc;
    match d.build() {
        Ok(()) => 1,
        Err(e) => {
            set_error(e);
            0
        }
    }
}

/// # Safety
/// `doc` must come from `gltf_open`, after a successful `gltf_build`.
#[no_mangle]
pub unsafe extern "C" fn gltf_result(doc: *mut Doc) -> *const BuildResult {
    let d = &*doc;
    &d.result
}

/// # Safety
/// `doc` must come from `gltf_open` and not have been closed.
#[no_mangle]
pub unsafe extern "C" fn gltf_close(doc: *mut Doc) {
    drop(Box::from_raw(doc));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base64_encode(bytes: &[u8]) -> String {
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = (chunk[0] as u32) << 16 | (chunk.get(1).copied().unwrap_or(0) as u32) << 8 | chunk.get(2).copied().unwrap_or(0) as u32;
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    fn floats(d: &Decoded) -> Vec<f32> {
        assert_eq!(d.component_type, accessors::FLOAT);
        d.data.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
    }

    fn attribute(doc: &Doc, mesh: usize, name: &str) -> usize {
        doc.json["meshes"][mesh]["primitives"][0]["attributes"][name].as_u64().unwrap() as usize
    }

    #[test]
    fn base64_roundtrip() {
        for n in 0..20 {
            let bytes: Vec<u8> = (0..n).map(|i| (i * 37 % 256) as u8).collect();
            assert_eq!(base64_decode(base64_encode(&bytes).as_bytes()).unwrap(), bytes);
        }
    }

    #[test]
    fn embedded_gltf_2() {
        let data = std::fs::read("../../assets/models/gltf/Box/glTF-Embedded/Box.gltf").unwrap();
        let mut doc = Doc::open(data).unwrap();
        assert!(doc.external_buffers().is_empty());
        assert!(doc.json["buffers"][0].get("uri").is_none(), "data URI stripped");
        doc.build().unwrap();
        let pos = floats(&doc.decoded[attribute(&doc, 0, "POSITION")]);
        assert_eq!(pos.len(), 24 * 3);
        assert!(pos.iter().all(|v| v.abs() <= 0.5 + 1e-6));
        let idx = &doc.decoded[doc.json["meshes"][0]["primitives"][0]["indices"].as_u64().unwrap() as usize];
        assert_eq!((idx.component_type, idx.count), (5123, 36));
    }

    #[test]
    fn external_buffer_quantized_and_texture_transform() {
        let data = std::fs::read("../../assets/models/gltf/Duck-Quantized/Duck.gltf").unwrap();
        let mut doc = Doc::open(data).unwrap();
        let external = doc.external_buffers();
        assert_eq!(external.len(), 1);
        assert_eq!(external[0].1, "Duck.bin");
        doc.set_buffer(0, std::fs::read("../../assets/models/gltf/Duck-Quantized/Duck.bin").unwrap()).unwrap();
        let before = doc.json["accessors"].as_array().unwrap().len();
        doc.build().unwrap();
        assert_eq!(doc.json["accessors"].as_array().unwrap().len(), before + 1, "transformed TEXCOORD_0 appended");
        let normals = floats(&doc.decoded[attribute(&doc, 0, "NORMAL")]);
        for n in normals.chunks(3).take(50) {
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            assert!((len - 1.0).abs() < 0.02, "dequantized normal has length {}", len);
        }
        let uv = &doc.decoded[attribute(&doc, 0, "TEXCOORD_0")];
        assert_eq!(uv.component_type, accessors::FLOAT);
        assert!(floats(uv).iter().all(|v| (-0.01..=1.01).contains(v)));
        let positions = &doc.decoded[attribute(&doc, 0, "POSITION")];
        assert_eq!(positions.component_type, 5123, "non-normalized quantized positions keep their type");
        assert_eq!(doc.image_table.len(), 1);
        assert_eq!(doc.json["images"][0]["uri"].as_str(), Some("DuckCM.png"));
        assert_eq!(doc.image_data[0].len(), 0, "external image left to the caller");
    }

    #[test]
    fn glb_2_with_embedded_image() {
        let data = std::fs::read("../../assets/models/gltf/BoxTextured/glTF-Binary/BoxTextured.glb").unwrap();
        let mut doc = Doc::open(data).unwrap();
        assert_eq!(doc.glb_version, 2);
        doc.build().unwrap();
        assert_eq!(doc.image_data.len(), 1);
        assert!(doc.image_data[0].len() > 1000);
        assert_eq!(doc.json["images"][0]["mimeType"].as_str(), Some("image/png"));
        assert_eq!(doc.decoded.len(), doc.json["accessors"].as_array().unwrap().len());
    }

    fn triangle_v1(with_common: bool) -> String {
        let positions: Vec<u8> = [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0].iter().flat_map(|v| v.to_le_bytes()).collect();
        let material = if with_common {
            r#"{"extensions":{"KHR_materials_common":{"technique":"PHONG","values":{"diffuse":[0.2,0.4,0.6,1],"transparency":0.5}}}}"#.to_string()
        } else {
            r#"{"technique":"t","values":{"diffuse":[0.2,0.4,0.6,1]}}"#.to_string()
        };
        format!(
            r#"{{"asset":{{"version":"1.0"}},"scene":"s","scenes":{{"s":{{"nodes":["n"]}}}},"nodes":{{"n":{{"meshes":["m"],"name":"Tri"}}}},
            "meshes":{{"m":{{"primitives":[{{"attributes":{{"POSITION":"a","TEXCOORD":"a"}},"material":"mat","primitive":4}}]}}}},
            "materials":{{"mat":{}}},"accessors":{{"a":{{"bufferView":"bv","byteOffset":0,"byteStride":12,"componentType":5126,"count":3,"type":"VEC3"}}}},
            "bufferViews":{{"bv":{{"buffer":"b","byteOffset":0,"byteLength":36}}}},"buffers":{{"b":{{"byteLength":36,"type":"arraybuffer","uri":"data:application/octet-stream;base64,{}"}}}}}}"#,
            material,
            base64_encode(&positions)
        )
    }

    #[test]
    fn gltf_1_normalization() {
        for with_common in [false, true] {
            let mut doc = Doc::open(triangle_v1(with_common).into_bytes()).unwrap();
            doc.build().unwrap();
            let j = &doc.json;
            assert_eq!(j["asset"]["version"], "2.0");
            assert_eq!(j["asset"]["_originalVersion"], "1.0");
            assert_eq!(j["scene"], 0);
            assert_eq!(j["scenes"][0]["nodes"][0], 0);
            assert_eq!(j["nodes"][0]["meshes"][0], 0);
            assert_eq!(j["nodes"][0]["name"], "Tri");
            let prim = &j["meshes"][0]["primitives"][0];
            assert_eq!(prim["mode"], 4);
            assert_eq!(prim["material"], 0);
            assert_eq!(prim["attributes"]["TEXCOORD_0"], 0);
            assert_eq!(j["materials"][0]["pbrMetallicRoughness"]["baseColorFactor"][1], 0.4);
            if with_common {
                assert_eq!(j["materials"][0]["pbrMetallicRoughness"]["baseColorFactor"][3], 0.5);
                assert_eq!(j["materials"][0]["alphaMode"], "BLEND");
            }
            assert_eq!(floats(&doc.decoded[0]), vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        }
    }

    #[test]
    fn sparse_accessor() {
        // 4 SCALAR u16 values [0,1,2,3] with sparse replacement of indices 1 and 3 by 100 and 200
        let base: Vec<u8> = [0u16, 1, 2, 3].iter().flat_map(|v| v.to_le_bytes()).collect();
        let idx: Vec<u8> = [1u8, 3].to_vec();
        let vals: Vec<u8> = [100u16, 200].iter().flat_map(|v| v.to_le_bytes()).collect();
        let mut bytes = base.clone();
        bytes.extend(&idx);
        bytes.extend(&vals);
        let json = format!(
            r#"{{"asset":{{"version":"2.0"}},"buffers":[{{"byteLength":{},"uri":"data:;base64,{}"}}],
            "bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":8}},{{"buffer":0,"byteOffset":8,"byteLength":2}},{{"buffer":0,"byteOffset":10,"byteLength":4}}],
            "accessors":[{{"bufferView":0,"componentType":5123,"count":4,"type":"SCALAR","normalized":true,
            "sparse":{{"count":2,"indices":{{"bufferView":1,"componentType":5121}},"values":{{"bufferView":2}}}}}}]}}"#,
            bytes.len(),
            base64_encode(&bytes)
        );
        let mut doc = Doc::open(json.into_bytes()).unwrap();
        doc.build().unwrap();
        let v = floats(&doc.decoded[0]);
        assert_eq!(v, vec![0.0, 100.0 / 65535.0, 2.0 / 65535.0, 200.0 / 65535.0]);
    }

    #[test]
    fn rejects_draco_required() {
        let json = r#"{"asset":{"version":"2.0"},"extensionsRequired":["KHR_draco_mesh_compression"]}"#;
        assert!(Doc::open(json.as_bytes().to_vec()).err().expect("error").contains("KHR_draco_mesh_compression"));
    }

    #[test]
    fn missing_external_buffer_is_an_error() {
        let json = r#"{"asset":{"version":"2.0"},"buffers":[{"byteLength":4,"uri":"missing.bin"}]}"#;
        let mut doc = Doc::open(json.as_bytes().to_vec()).unwrap();
        assert_eq!(doc.external_buffers(), vec![(0, "missing.bin".to_string())]);
        assert!(doc.build().unwrap_err().contains("buffer 0"));
    }
}
