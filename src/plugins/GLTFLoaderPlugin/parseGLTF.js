import {gltfWasm} from "./gltfWasm.js";
import {copyFromWasm, copyToWasm, loadWasm, readWasmError, readWasmString} from "../lib/wasm/loadWasm.js";

/**
 * glTF parser for {@link GLTFLoaderPlugin}, backed by the WebAssembly module built from wasm/gltf.
 *
 * The module parses glTF 1.0 (including KHR_binary_glTF / GLB version 1 and KHR_materials_common) and glTF 2.x
 * (JSON or GLB version 2), decodes accessors (byte stride, sparse, dequantization of normalized attributes), bakes
 * KHR_texture_transform into TEXCOORD_0 and extracts embedded image bytes. This side fetches external buffers and
 * images, decodes images, and links the index references into an object graph. Meshes requiring
 * KHR_draco_mesh_compression or EXT_meshopt_compression are rejected with an error.
 *
 * @private
 */

const DEFAULT_SAMPLER = {magFilter: 9729, minFilter: 9987, wrapS: 10497, wrapT: 10497};

const COMPONENT_ARRAYS = {
    5120: Int8Array,
    5121: Uint8Array,
    5122: Int16Array,
    5123: Uint16Array,
    5125: Uint32Array,
    5126: Float32Array
};

/**
 * @param {ArrayBuffer|Uint8Array|String|Object} data glTF JSON (object or text), or a GLB / JSON text ArrayBuffer.
 * @param {Object} [options]
 * @param {String} [options.src] URL of the glTF asset, for resolving relative URIs through the data source.
 * @param {Object} [options.dataSource] Object with getArrayBuffer(src, uri, ok, error), as {@link GLTFDefaultDataSource}.
 * @param {Function} [options.log] Receives warning messages.
 * @returns {Promise<Object>} The resolved glTF.
 */
export async function parseGLTF(data, options = {}) {
    const log = options.log || ((msg) => console.warn(msg));
    const wasm = await loadWasm(gltfWasm);
    const exports = wasm.exports;
    const bytes = toBytes(data);
    const doc = exports.gltf_open(copyToWasm(wasm, bytes), bytes.length);
    if (!doc) {
        throw new Error(readWasmError(wasm));
    }
    try {
        let json = JSON.parse(readWasmString(wasm, exports.gltf_json_ptr(doc), exports.gltf_json_len(doc)));
        // External buffers
        await Promise.all((json.buffers || []).map(async (buffer, i) => {
            if (buffer.uri !== undefined) {
                const bufferBytes = await fetchURI(buffer.uri, options);
                if (!exports.gltf_set_buffer(doc, i, copyToWasm(wasm, bufferBytes), bufferBytes.length)) {
                    throw new Error(readWasmError(wasm));
                }
            }
        }));
        if (!exports.gltf_build(doc)) {
            throw new Error(readWasmError(wasm));
        }
        const result = new Uint32Array(exports.memory.buffer, exports.gltf_result(doc), 6);
        json = JSON.parse(readWasmString(wasm, result[0], result[1]));
        const accessorTable = new Uint32Array(exports.memory.buffer, result[2], result[3] * 6);
        const imageTable = new Uint32Array(exports.memory.buffer, result[4], result[5] * 2);
        const gltf = {json};
        gltf.accessors = (json.accessors || []).map((accessor, i) => {
            const componentType = accessorTable[i * 6];
            const ArrayType = COMPONENT_ARRAYS[componentType];
            const value = new ArrayType(copyFromWasm(wasm, accessorTable[i * 6 + 3], accessorTable[i * 6 + 4]));
            return {...accessor, componentType, components: accessorTable[i * 6 + 1], normalized: !!accessorTable[i * 6 + 5], value};
        });
        gltf.images = await loadImages(gltf, wasm, imageTable, options, log);
        gltf.samplers = (json.samplers || []).map((s) => ({...DEFAULT_SAMPLER, ...s}));
        gltf.textures = (json.textures || []).map((t) => resolveTexture(gltf, t));
        gltf.materials = (json.materials || []).map((m) => resolveMaterial(gltf, m));
        gltf.meshes = (json.meshes || []).map((m) => resolveMesh(gltf, m));
        gltf.nodes = (json.nodes || []).map((n) => ({...n}));
        gltf.nodes.forEach((node, i) => resolveNode(gltf, node, json.nodes[i]));
        gltf.scenes = (json.scenes || []).map((s) => ({...s, nodes: (s.nodes || []).map((i) => gltf.nodes[i])}));
        gltf.scene = (typeof json.scene === "number") ? gltf.scenes[json.scene] : undefined;
        gltf.asset = json.asset;
        gltf.extensionsUsed = json.extensionsUsed;
        gltf.extensionsRequired = json.extensionsRequired;
        return gltf;
    } finally {
        exports.gltf_close(doc);
    }
}

function toBytes(data) {
    if (data instanceof ArrayBuffer) {
        return new Uint8Array(data);
    }
    if (ArrayBuffer.isView(data)) {
        return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
    }
    if (typeof data === "string") {
        return new TextEncoder().encode(data);
    }
    if (data && typeof data === "object") {
        return new TextEncoder().encode(JSON.stringify(data));
    }
    throw new Error("glTF: expected an ArrayBuffer, a JSON string or a JSON object");
}

/**
 * Fetches the bytes of a URI relative to the glTF asset, through the data source when given.
 */
function fetchURI(uri, options) {
    if (options.dataSource && options.dataSource.getArrayBuffer) {
        return new Promise((resolve, reject) => {
            options.dataSource.getArrayBuffer(options.src || "", uri, (arrayBuffer) => resolve(new Uint8Array(arrayBuffer)), reject);
        });
    }
    const base = options.src || "";
    const url = /^[a-z][a-z0-9+.-]*:/i.test(uri) ? uri : base.substring(0, base.lastIndexOf("/") + 1) + uri;
    return fetch(url).then((response) => {
        if (!response.ok) {
            throw new Error(`${response.status} ${response.statusText} loading ${url}`);
        }
        return response.arrayBuffer();
    }).then((arrayBuffer) => new Uint8Array(arrayBuffer));
}

/** Loads every image referenced by a texture (directly or through a texture source extension). */
async function loadImages(gltf, wasm, imageTable, options, log) {
    const json = gltf.json;
    const images = (json.images || []).map((image) => ({...image}));
    const referenced = new Set();
    for (const texture of (json.textures || [])) {
        const source = textureSource(texture);
        if (source !== undefined) {
            referenced.add(source);
        }
    }
    await Promise.all(Array.from(referenced).map(async (i) => {
        const image = images[i];
        if (!image) {
            return;
        }
        try {
            let bytes;
            if (imageTable[i * 2 + 1] > 0) {
                bytes = new Uint8Array(copyFromWasm(wasm, imageTable[i * 2], imageTable[i * 2 + 1]));
            } else if (image.uri !== undefined) {
                bytes = await fetchURI(image.uri, options);
            } else {
                throw new Error("image has neither uri nor bufferView");
            }
            image.data = bytes;
            if (image.mimeType === "image/ktx2" || image.mimeType === "image/basis") {
                image.buffers = [bytes.buffer]; // Left to the SceneModel's texture transcoder
            } else {
                image.image = await decodeImage(bytes, image.mimeType);
            }
        } catch (e) {
            log(`glTF: failed to load image ${i}${image.uri ? ` (${image.uri})` : ""} - ${e.message || e}`);
        }
    }));
    return images;
}

/** Decodes image bytes into an ImageBitmap (or an HTMLImageElement where ImageBitmap is unavailable). */
function decodeImage(bytes, mimeType) {
    if (typeof Blob === "undefined" || (typeof createImageBitmap === "undefined" && typeof Image === "undefined")) {
        return Promise.resolve(undefined); // Not a browser - the raw bytes stay in image.data
    }
    const blob = new Blob([bytes], {type: mimeType || ""});
    if (typeof createImageBitmap !== "undefined") {
        return createImageBitmap(blob, {colorSpaceConversion: "none", premultiplyAlpha: "none"});
    }
    return new Promise((resolve, reject) => {
        const url = URL.createObjectURL(blob);
        const image = new Image();
        image.onload = () => {
            URL.revokeObjectURL(url);
            resolve(image);
        };
        image.onerror = () => {
            URL.revokeObjectURL(url);
            reject(new Error("image decoding failed"));
        };
        image.src = url;
    });
}

/** The image index of a texture, preferring the core source over extension sources when both exist. */
function textureSource(texture) {
    if (texture.source !== undefined) {
        return texture.source;
    }
    const ext = texture.extensions || {};
    for (const name of ["KHR_texture_basisu", "EXT_texture_webp"]) {
        if (ext[name] && ext[name].source !== undefined) {
            return ext[name].source;
        }
    }
    return undefined;
}

function resolveTexture(gltf, texture) {
    const source = textureSource(texture);
    return {
        ...texture,
        sampler: (typeof texture.sampler === "number" && gltf.samplers[texture.sampler]) || DEFAULT_SAMPLER,
        source: (source !== undefined) ? gltf.images[source] : undefined
    };
}

function resolveMaterial(gltf, material) {
    const m = {...material};
    const link = (info) => (info && typeof info.index === "number") ? {...info, texture: gltf.textures[info.index]} : info;
    m.normalTexture = link(m.normalTexture);
    m.occlusionTexture = link(m.occlusionTexture);
    m.emissiveTexture = link(m.emissiveTexture);
    if (m.pbrMetallicRoughness) {
        m.pbrMetallicRoughness = {...m.pbrMetallicRoughness};
        m.pbrMetallicRoughness.baseColorTexture = link(m.pbrMetallicRoughness.baseColorTexture);
        m.pbrMetallicRoughness.metallicRoughnessTexture = link(m.pbrMetallicRoughness.metallicRoughnessTexture);
    }
    return m;
}

function resolveMesh(gltf, mesh) {
    return {
        ...mesh,
        primitives: (mesh.primitives || []).map((p) => {
            const attributes = {};
            for (const name of Object.keys(p.attributes || {})) {
                attributes[name] = gltf.accessors[p.attributes[name]];
            }
            return {
                ...p,
                mode: (p.mode !== undefined) ? p.mode : 4,
                attributes,
                indices: (p.indices !== undefined) ? gltf.accessors[p.indices] : undefined,
                material: (p.material !== undefined) ? gltf.materials[p.material] : undefined
            };
        })
    };
}

function resolveNode(gltf, node, source) {
    node.children = (source.children || []).map((i) => gltf.nodes[i]).filter((n) => !!n);
    if (source.mesh !== undefined) {
        node.mesh = gltf.meshes[source.mesh];
    } else if (source.meshes && source.meshes.length > 0) { // glTF 1.0
        node.mesh = {primitives: [].concat(...source.meshes.map((i) => gltf.meshes[i].primitives))};
    }
}
