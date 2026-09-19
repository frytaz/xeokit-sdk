//! Accessor decoding (byte stride, sparse substitution, dequantization) and KHR_texture_transform baking.

use serde_json::{Map, Value};

pub const FLOAT: u32 = 5126;

pub struct View {
    pub buffer: usize,
    pub offset: usize,
    pub length: usize,
    pub stride: usize,
}

/// A decoded accessor: contiguous little-endian values of `component_type`.
pub struct Decoded {
    pub component_type: u32,
    pub components: usize,
    pub count: usize,
    pub normalized: bool,
    pub data: Vec<u8>,
}

pub fn component_size(component_type: u32) -> Option<usize> {
    match component_type {
        5120 | 5121 => Some(1),
        5122 | 5123 => Some(2),
        5125 | 5126 => Some(4),
        _ => None,
    }
}

pub fn type_components(t: &str) -> Option<usize> {
    match t {
        "SCALAR" => Some(1),
        "VEC2" => Some(2),
        "VEC3" => Some(3),
        "VEC4" | "MAT2" => Some(4),
        "MAT3" => Some(9),
        "MAT4" => Some(16),
        _ => None,
    }
}

fn u(v: &Value, key: &str) -> Option<usize> {
    v.get(key).and_then(|x| x.as_u64()).map(|x| x as usize)
}

pub fn resolve_views(json: &Value, buffers: &[&[u8]]) -> Result<Vec<View>, String> {
    let mut views = Vec::new();
    for (i, bv) in json.get("bufferViews").and_then(|v| v.as_array()).into_iter().flatten().enumerate() {
        let buffer = u(bv, "buffer").ok_or(format!("glTF: bufferView {} has no buffer", i))?;
        let data = buffers.get(buffer).ok_or(format!("glTF: bufferView {} references unknown buffer {}", i, buffer))?;
        let offset = u(bv, "byteOffset").unwrap_or(0);
        let length = u(bv, "byteLength").unwrap_or(data.len().saturating_sub(offset));
        if offset + length > data.len() {
            return Err(format!("glTF: bufferView {} exceeds its buffer", i));
        }
        views.push(View { buffer, offset, length, stride: u(bv, "byteStride").unwrap_or(0) });
    }
    Ok(views)
}

/// Copies `count` elements of `components` components of `size` bytes each, honoring the byte stride.
fn read_elements(data: &[u8], byte_offset: usize, stride: usize, count: usize, components: usize, size: usize) -> Result<Vec<u8>, String> {
    let element_bytes = components * size;
    let stride = if stride == 0 { element_bytes } else { stride };
    if count == 0 {
        return Ok(Vec::new());
    }
    if byte_offset + (count - 1) * stride + element_bytes > data.len() {
        return Err("glTF: accessor exceeds its bufferView".into());
    }
    let mut out = Vec::with_capacity(count * element_bytes);
    if stride == element_bytes {
        out.extend_from_slice(&data[byte_offset..byte_offset + count * element_bytes]);
    } else {
        for i in 0..count {
            let off = byte_offset + i * stride;
            out.extend_from_slice(&data[off..off + element_bytes]);
        }
    }
    Ok(out)
}

fn view_data<'a>(views: &[View], buffers: &[&'a [u8]], index: usize) -> Result<&'a [u8], String> {
    let view = views.get(index).ok_or(format!("glTF: unknown bufferView {}", index))?;
    Ok(&buffers[view.buffer][view.offset..view.offset + view.length])
}

/// Converts normalized integers to floats, per the glTF specification.
fn dequantize(data: &[u8], component_type: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() / component_size(component_type).unwrap_or(1) * 4);
    let mut push = |f: f32| out.extend_from_slice(&f.to_le_bytes());
    match component_type {
        5120 => data.iter().for_each(|&b| push((b as i8 as f32 / 127.0).max(-1.0))),
        5121 => data.iter().for_each(|&b| push(b as f32 / 255.0)),
        5122 => data.chunks_exact(2).for_each(|c| push((i16::from_le_bytes([c[0], c[1]]) as f32 / 32767.0).max(-1.0))),
        5123 => data.chunks_exact(2).for_each(|c| push(u16::from_le_bytes([c[0], c[1]]) as f32 / 65535.0)),
        _ => return data.to_vec(),
    }
    out
}

pub fn decode(views: &[View], buffers: &[&[u8]], accessor: &Value, index: usize) -> Result<Decoded, String> {
    let component_type = accessor.get("componentType").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let size = component_size(component_type).ok_or(format!("glTF: accessor {} has unsupported componentType {}", index, component_type))?;
    let components = accessor.get("type").and_then(|v| v.as_str()).and_then(type_components).ok_or(format!("glTF: accessor {} has unsupported type", index))?;
    let count = u(accessor, "count").unwrap_or(0);
    let mut data = match u(accessor, "bufferView") {
        Some(bv) => {
            let view = views.get(bv).ok_or(format!("glTF: accessor {} references unknown bufferView {}", index, bv))?;
            let stride = if view.stride != 0 { view.stride } else { u(accessor, "byteStride").unwrap_or(0) }; // byteStride lives on the accessor in glTF 1.0
            read_elements(view_data(views, buffers, bv)?, u(accessor, "byteOffset").unwrap_or(0), stride, count, components, size).map_err(|e| format!("{} ({})", e, index))?
        }
        None => vec![0; count * components * size],
    };
    if let Some(sparse) = accessor.get("sparse") {
        let sparse_count = u(sparse, "count").unwrap_or(0);
        if sparse_count > 0 {
            let indices = sparse.get("indices").ok_or("glTF: sparse accessor without indices")?;
            let values = sparse.get("values").ok_or("glTF: sparse accessor without values")?;
            let indices_type = indices.get("componentType").and_then(|v| v.as_u64()).unwrap_or(5125) as u32;
            let isize_ = component_size(indices_type).ok_or("glTF: sparse indices have an unsupported componentType")?;
            let ibv = u(indices, "bufferView").ok_or("glTF: sparse indices without bufferView")?;
            let vbv = u(values, "bufferView").ok_or("glTF: sparse values without bufferView")?;
            let idx = read_elements(view_data(views, buffers, ibv)?, u(indices, "byteOffset").unwrap_or(0), 0, sparse_count, 1, isize_)?;
            let vals = read_elements(view_data(views, buffers, vbv)?, u(values, "byteOffset").unwrap_or(0), 0, sparse_count, components, size)?;
            let element_bytes = components * size;
            for k in 0..sparse_count {
                let target = match isize_ {
                    1 => idx[k] as usize,
                    2 => u16::from_le_bytes([idx[k * 2], idx[k * 2 + 1]]) as usize,
                    _ => u32::from_le_bytes([idx[k * 4], idx[k * 4 + 1], idx[k * 4 + 2], idx[k * 4 + 3]]) as usize,
                };
                if target >= count {
                    return Err(format!("glTF: sparse index {} out of range in accessor {}", target, index));
                }
                data[target * element_bytes..(target + 1) * element_bytes].copy_from_slice(&vals[k * element_bytes..(k + 1) * element_bytes]);
            }
        }
    }
    let normalized = accessor.get("normalized").and_then(|v| v.as_bool()).unwrap_or(false);
    if normalized && component_type != FLOAT {
        data = dequantize(&data, component_type);
        return Ok(Decoded { component_type: FLOAT, components, count, normalized: true, data });
    }
    Ok(Decoded { component_type, components, count, normalized, data })
}

fn as_f32s(d: &Decoded) -> Vec<f32> {
    match d.component_type {
        FLOAT => d.data.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect(),
        5120 => d.data.iter().map(|&b| b as i8 as f32).collect(),
        5121 => d.data.iter().map(|&b| b as f32).collect(),
        5122 => d.data.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32).collect(),
        5123 => d.data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]) as f32).collect(),
        _ => d.data.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f32).collect(),
    }
}

struct Transform {
    material: usize,
    m: [f64; 6],
    tex_coord: usize,
}

/// Bakes KHR_texture_transform of each material's color texture into the TEXCOORD_0 of the primitives using it,
/// appending the transformed coordinates as new accessors.
pub fn apply_texture_transforms(json: &mut Value, decoded: &mut Vec<Decoded>) -> Result<(), String> {
    let used = json.get("extensionsUsed").and_then(|v| v.as_array()).map(|a| a.iter().any(|e| e.as_str() == Some("KHR_texture_transform"))).unwrap_or(false);
    if !used {
        return Ok(());
    }
    let mut transforms = Vec::new();
    for (mi, material) in json.get("materials").and_then(|v| v.as_array()).into_iter().flatten().enumerate() {
        let pbr = material.get("pbrMetallicRoughness");
        let infos = [
            pbr.and_then(|p| p.get("baseColorTexture")),
            material.get("emissiveTexture"),
            material.get("normalTexture"),
            material.get("occlusionTexture"),
            pbr.and_then(|p| p.get("metallicRoughnessTexture")),
        ];
        for info in infos.into_iter().flatten() {
            if let Some(ext) = info.get("extensions").and_then(|e| e.get("KHR_texture_transform")) {
                let f = |v: Option<&Value>, i: usize, default: f64| v.and_then(|a| a.get(i)).and_then(|x| x.as_f64()).unwrap_or(default);
                let (ox, oy) = (f(ext.get("offset"), 0, 0.0), f(ext.get("offset"), 1, 0.0));
                let (sx, sy) = (f(ext.get("scale"), 0, 1.0), f(ext.get("scale"), 1, 1.0));
                let rotation = ext.get("rotation").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let (cos, sin) = (rotation.cos(), rotation.sin());
                // [u' v' 1] = T * R * S * [u v 1], with the rotation matrix as printed in the KHR_texture_transform
                // specification (verified against the Khronos TextureTransformTest sample)
                let tex_coord = ext.get("texCoord").and_then(|v| v.as_u64()).or_else(|| info.get("texCoord").and_then(|v| v.as_u64())).unwrap_or(0) as usize;
                transforms.push(Transform { material: mi, m: [cos * sx, sin * sy, ox, -sin * sx, cos * sy, oy], tex_coord });
                break;
            }
        }
    }
    if transforms.is_empty() {
        return Ok(());
    }
    let mut new_accessors: Vec<(usize, Vec<u8>)> = Vec::new(); // (count, data)
    let accessor_count = json.get("accessors").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
    for mesh in json.get_mut("meshes").and_then(|v| v.as_array_mut()).into_iter().flatten() {
        for primitive in mesh.get_mut("primitives").and_then(|v| v.as_array_mut()).into_iter().flatten().filter_map(|v| v.as_object_mut()) {
            let material = match primitive.get("material").and_then(|v| v.as_u64()) {
                Some(m) => m as usize,
                None => continue,
            };
            let t = match transforms.iter().find(|t| t.material == material) {
                Some(t) => t,
                None => continue,
            };
            let attributes = match primitive.get_mut("attributes").and_then(|v| v.as_object_mut()) {
                Some(a) => a,
                None => continue,
            };
            let source = attributes.get(&format!("TEXCOORD_{}", t.tex_coord)).or_else(|| attributes.get("TEXCOORD_0")).and_then(|v| v.as_u64()).map(|v| v as usize);
            let source = match source {
                Some(s) if s < decoded.len() => s,
                _ => continue,
            };
            let uv = as_f32s(&decoded[source]);
            let m = t.m;
            let mut out = Vec::with_capacity(uv.len() * 4);
            for pair in uv.chunks_exact(2) {
                let (u, v) = (pair[0] as f64, pair[1] as f64);
                out.extend_from_slice(&((m[0] * u + m[1] * v + m[2]) as f32).to_le_bytes());
                out.extend_from_slice(&((m[3] * u + m[4] * v + m[5]) as f32).to_le_bytes());
            }
            let count = decoded[source].count;
            let new_index = accessor_count + new_accessors.len();
            new_accessors.push((count, out));
            attributes.insert("TEXCOORD_0".into(), Value::from(new_index as u64));
        }
    }
    let accessors = json.get_mut("accessors").and_then(|v| v.as_array_mut()).ok_or("glTF: no accessors")?;
    for (count, data) in new_accessors {
        let mut a = Map::new();
        a.insert("componentType".into(), Value::from(FLOAT));
        a.insert("count".into(), Value::from(count as u64));
        a.insert("type".into(), Value::String("VEC2".into()));
        accessors.push(Value::Object(a));
        decoded.push(Decoded { component_type: FLOAT, components: 2, count, normalized: false, data });
    }
    Ok(())
}
