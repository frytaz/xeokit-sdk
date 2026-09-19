//! Converts the parts of a glTF 1.0 asset that the loader uses into glTF 2.0 structures, in place.

use serde_json::{Map, Value};
use std::collections::HashMap;

const COLLECTIONS: [&str; 13] = ["accessors", "animations", "buffers", "bufferViews", "cameras", "images", "materials", "meshes", "nodes", "samplers", "scenes", "skins", "textures"];

type Ids = HashMap<&'static str, HashMap<String, usize>>;

/// Looks up the array index of a glTF 1.0 object id.
fn index(ids: &Ids, collection: &str, id: &Value) -> Result<Option<u64>, String> {
    match id {
        Value::Null => Ok(None),
        Value::Number(n) => Ok(n.as_u64()),
        Value::String(s) => match ids.get(collection).and_then(|m| m.get(s)) {
            Some(&i) => Ok(Some(i as u64)),
            None => Err(format!("glTF 1.0: unknown {} id \"{}\"", collection, s)),
        },
        _ => Err(format!("glTF 1.0: invalid {} reference", collection)),
    }
}

/// Replaces the id stored under `key` (if any) with its array index.
fn remap(obj: &mut Map<String, Value>, key: &str, collection: &str, ids: &Ids) -> Result<(), String> {
    if let Some(v) = obj.get(key) {
        match index(ids, collection, v)? {
            Some(i) => {
                obj.insert(key.to_string(), Value::from(i));
            }
            None => {
                obj.remove(key);
            }
        }
    }
    Ok(())
}

fn remap_array(obj: &mut Map<String, Value>, key: &str, collection: &str, ids: &Ids) -> Result<(), String> {
    if let Some(Value::Array(items)) = obj.get(key) {
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            if let Some(i) = index(ids, collection, item)? {
                out.push(Value::from(i));
            }
        }
        obj.insert(key.to_string(), Value::Array(out));
    }
    Ok(())
}

fn objects_mut<'a>(root: &'a mut Map<String, Value>, name: &str) -> impl Iterator<Item = &'a mut Map<String, Value>> {
    root.get_mut(name).and_then(|v| v.as_array_mut()).into_iter().flatten().filter_map(|v| v.as_object_mut())
}

pub fn normalize(json: &mut Value) -> Result<(), String> {
    let root = json.as_object_mut().ok_or("glTF: root is not an object")?;
    let mut ids: Ids = HashMap::new();
    for name in COLLECTIONS {
        let mut map = HashMap::new();
        match root.get_mut(name) {
            Some(Value::Object(dict)) => {
                let dict = std::mem::take(dict);
                let mut array = Vec::with_capacity(dict.len());
                for (id, mut item) in dict {
                    if let Value::Object(obj) = &mut item {
                        if !obj.contains_key("name") {
                            obj.insert("name".into(), Value::String(id.clone()));
                        }
                    }
                    map.insert(id, array.len());
                    array.push(item);
                }
                root.insert(name.into(), Value::Array(array));
            }
            Some(Value::Array(_)) => {}
            _ => {
                root.insert(name.into(), Value::Array(Vec::new()));
            }
        }
        ids.insert(name, map);
    }
    for buffer in objects_mut(root, "buffers") {
        let name = buffer.get("name").and_then(|v| v.as_str()).unwrap_or("");
        if name == "binary_glTF" || name == "KHR_binary_glTF" {
            buffer.insert("_glbBinary".into(), Value::Bool(true));
            buffer.remove("uri");
        }
        buffer.remove("type");
    }
    for bv in objects_mut(root, "bufferViews") {
        remap(bv, "buffer", "buffers", &ids)?;
    }
    for accessor in objects_mut(root, "accessors") {
        remap(accessor, "bufferView", "bufferViews", &ids)?;
    }
    for image in objects_mut(root, "images") {
        let binary = image.get("extensions").and_then(|e| e.get("KHR_binary_glTF")).cloned();
        if let Some(Value::Object(binary)) = binary {
            if let Some(bv) = binary.get("bufferView") {
                if let Some(i) = index(&ids, "bufferViews", bv)? {
                    image.insert("bufferView".into(), Value::from(i));
                }
            }
            if let Some(mime) = binary.get("mimeType") {
                image.insert("mimeType".into(), mime.clone());
            }
            image.remove("uri");
        }
    }
    for texture in objects_mut(root, "textures") {
        remap(texture, "sampler", "samplers", &ids)?;
        remap(texture, "source", "images", &ids)?;
    }
    for material in objects_mut(root, "materials") {
        normalize_material(material, &ids)?;
    }
    for mesh in objects_mut(root, "meshes") {
        for primitive in mesh.get_mut("primitives").and_then(|v| v.as_array_mut()).into_iter().flatten().filter_map(|v| v.as_object_mut()) {
            let mut attributes = Map::new();
            if let Some(Value::Object(attrs)) = primitive.get("attributes") {
                for (semantic, id) in attrs {
                    let name = match semantic.as_str() {
                        "TEXCOORD" => "TEXCOORD_0".to_string(),
                        "COLOR" => "COLOR_0".to_string(),
                        other => other.to_string(),
                    };
                    if let Some(i) = index(&ids, "accessors", id)? {
                        attributes.insert(name, Value::from(i));
                    }
                }
            }
            primitive.insert("attributes".into(), Value::Object(attributes));
            remap(primitive, "indices", "accessors", &ids)?;
            remap(primitive, "material", "materials", &ids)?;
            if !primitive.contains_key("mode") {
                if let Some(mode) = primitive.get("primitive").cloned() {
                    primitive.insert("mode".into(), mode);
                }
            }
        }
    }
    for node in objects_mut(root, "nodes") {
        remap_array(node, "children", "nodes", &ids)?;
        remap_array(node, "meshes", "meshes", &ids)?;
        remap(node, "camera", "cameras", &ids)?;
        remap(node, "skin", "skins", &ids)?;
    }
    for scene in objects_mut(root, "scenes") {
        remap_array(scene, "nodes", "nodes", &ids)?;
    }
    remap(root, "scene", "scenes", &ids)?;
    let original_version = root.get("asset").and_then(|a| a.get("version")).cloned().unwrap_or(Value::String("1.0".into()));
    let mut asset = root.get("asset").and_then(|a| a.as_object()).cloned().unwrap_or_default();
    asset.insert("version".into(), Value::String("2.0".into()));
    asset.insert("_originalVersion".into(), original_version);
    root.insert("asset".into(), Value::Object(asset));
    Ok(())
}

/// Maps a glTF 1.0 material (technique values or KHR_materials_common) to pbrMetallicRoughness.
fn normalize_material(material: &mut Map<String, Value>, ids: &Ids) -> Result<(), String> {
    let common = material.get("extensions").and_then(|e| e.get("KHR_materials_common")).cloned();
    let values = common.as_ref().and_then(|c| c.get("values")).cloned().or_else(|| material.get("values").cloned()).unwrap_or(Value::Object(Map::new()));
    let mut pbr = match material.get("pbrMetallicRoughness") {
        Some(Value::Object(p)) => p.clone(),
        _ => {
            let mut p = Map::new();
            p.insert("metallicFactor".into(), Value::from(0));
            p.insert("roughnessFactor".into(), Value::from(1));
            p
        }
    };
    match values.get("diffuse") {
        Some(Value::Array(d)) if d.len() >= 3 => {
            let alpha = d.get(3).cloned().unwrap_or(Value::from(1));
            pbr.insert("baseColorFactor".into(), Value::Array(vec![d[0].clone(), d[1].clone(), d[2].clone(), alpha]));
        }
        Some(Value::String(id)) => {
            if let Some(i) = index(ids, "textures", &Value::String(id.clone()))? {
                let mut info = Map::new();
                info.insert("index".into(), Value::from(i));
                pbr.insert("baseColorTexture".into(), Value::Object(info));
            }
        }
        _ => {}
    }
    if let Some(t) = values.get("transparency").and_then(|v| v.as_f64()) {
        let mut factor = pbr.get("baseColorFactor").and_then(|v| v.as_array()).cloned().unwrap_or_else(|| vec![Value::from(1), Value::from(1), Value::from(1), Value::from(1)]);
        while factor.len() < 4 {
            factor.push(Value::from(1));
        }
        factor[3] = Value::from(t);
        pbr.insert("baseColorFactor".into(), Value::Array(factor));
        if t < 1.0 {
            material.insert("alphaMode".into(), Value::String("BLEND".into()));
        }
    }
    if let Some(ds) = common.as_ref().and_then(|c| c.get("doubleSided")).and_then(|v| v.as_bool()) {
        material.insert("doubleSided".into(), Value::Bool(ds));
    }
    material.insert("pbrMetallicRoughness".into(), Value::Object(pbr));
    Ok(())
}
