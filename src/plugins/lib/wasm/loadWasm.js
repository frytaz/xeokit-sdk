/**
 * Instantiates a WebAssembly module embedded as base64 (see wasm/build.mjs), once per module.
 * @private
 */

const instances = new Map();
const modules = new Map();

/**
 * @param {String} base64 The module bytes as base64.
 * @returns {Promise<WebAssembly.Instance>}
 */
export function loadWasm(base64) {
    let promise = instances.get(base64);
    if (!promise) {
        promise = WebAssembly.instantiate(decodeBase64(base64), {}).then((result) => result.instance);
        instances.set(base64, promise);
    }
    return promise;
}

/**
 * Compiles a module without instantiating it, once per module; the result can be posted to a Web Worker.
 * @param {String} base64 The module bytes as base64.
 * @returns {Promise<WebAssembly.Module>}
 */
export function compileWasm(base64) {
    let promise = modules.get(base64);
    if (!promise) {
        promise = WebAssembly.compile(decodeBase64(base64));
        modules.set(base64, promise);
    }
    return promise;
}

/* global Buffer */
function decodeBase64(base64) {
    if (typeof Buffer !== "undefined" && Buffer.from) { // Node.js
        const buf = Buffer.from(base64, "base64");
        return new Uint8Array(buf.buffer, buf.byteOffset, buf.byteLength);
    }
    const binary = atob(base64);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) {
        bytes[i] = binary.charCodeAt(i);
    }
    return bytes;
}

/**
 * Copies bytes into the module's memory. Returns the pointer; the callee takes ownership or the caller must `dealloc`.
 */
export function copyToWasm(instance, bytes) {
    const ptr = instance.exports.alloc(bytes.length);
    new Uint8Array(instance.exports.memory.buffer, ptr, bytes.length).set(bytes);
    return ptr;
}

/** Copies `len` bytes out of the module's memory into a fresh ArrayBuffer. */
export function copyFromWasm(instance, ptr, len) {
    return instance.exports.memory.buffer.slice(ptr, ptr + len);
}

const textDecoder = new TextDecoder();

export function readWasmString(instance, ptr, len) {
    return textDecoder.decode(new Uint8Array(instance.exports.memory.buffer, ptr, len));
}

/** The module's last error message. */
export function readWasmError(instance) {
    return readWasmString(instance, instance.exports.error_ptr(), instance.exports.error_len());
}
