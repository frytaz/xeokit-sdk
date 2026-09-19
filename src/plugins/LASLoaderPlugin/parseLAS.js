import {lasWasm} from "./lasWasm.js";
import {copyFromWasm, copyToWasm, loadWasm, readWasmError, readWasmString} from "../lib/wasm/loadWasm.js";

// Info struct written by the wasm module (see wasm/las/src/lib.rs, `Info`): 15 little-endian u32 fields
const INFO_NUM_KEPT = 0, INFO_POINT_FORMAT = 2, INFO_FP64 = 3, INFO_POSITIONS_PTR = 4, INFO_POSITIONS_LEN = 5,
    INFO_COLORS_PTR = 6, INFO_COLORS_LEN = 7, INFO_INTENSITIES_PTR = 8, INFO_INTENSITIES_LEN = 9,
    INFO_CLASSIFICATIONS_PTR = 10, INFO_CLASSIFICATIONS_LEN = 11, INFO_HEADER_PTR = 12, INFO_HEADER_LEN = 13;

const POINTS_PER_CALL = 200000;

/**
 * Parses a LAS or LAZ (LASzip-compressed) file with the WebAssembly decoder built from wasm/las.
 *
 * Supports LAS 1.0 - 1.4 (R15), point data record formats 0 - 10, with or without LASzip compression.
 *
 * @private
 * @param {ArrayBuffer} arrayBuffer The LAS/LAZ file.
 * @param {Object} [options]
 * @param {Number} [options.skip=1] Load every n-th point.
 * @param {Boolean} [options.fp64=false] Return positions as Float64Array instead of Float32Array.
 * @param {Number|String} [options.colorDepth="auto"] 8, 16 or "auto" - whether colors are stored in 8 or 16 bits.
 * @returns {Promise<{header: Object, pointsFormatId: Number, numPoints: Number, positions: Float32Array|Float64Array, colors: Uint8Array|null, intensities: Uint16Array, classifications: Uint8Array}>}
 * `colors` holds 8-bit RGB triples, `intensities` the raw 16-bit intensities, `header` the public header block
 * and (when found) the `epsg` code from the GeoTIFF projection VLR.
 */
export async function parseLAS(arrayBuffer, options = {}) {
    const wasm = await loadWasm(lasWasm);
    const exports = wasm.exports;
    const bytes = new Uint8Array(arrayBuffer);
    const ptr = copyToWasm(wasm, bytes);
    const colorDepth = (options.colorDepth === 8 || options.colorDepth === 16) ? options.colorDepth : 0;
    const reader = exports.las_open(ptr, bytes.length, Math.max(1, Math.floor(options.skip || 1)), options.fp64 ? 1 : 0, colorDepth);
    if (!reader) {
        throw new Error(readWasmError(wasm));
    }
    try {
        let lastYield = Date.now();
        for (; ;) {
            const remaining = exports.las_read(reader, POINTS_PER_CALL);
            if (remaining === 0xFFFFFFFF) {
                throw new Error(readWasmError(wasm));
            }
            if (remaining === 0) {
                break;
            }
            if (Date.now() - lastYield > 50) { // Keep the browser responsive while decoding large files
                await new Promise((resolve) => setTimeout(resolve, 0));
                lastYield = Date.now();
            }
        }
        const info = new Uint32Array(exports.memory.buffer, exports.las_info(reader), 15);
        const fp64 = info[INFO_FP64] !== 0;
        const positions = new (fp64 ? Float64Array : Float32Array)(copyFromWasm(wasm, info[INFO_POSITIONS_PTR], info[INFO_POSITIONS_LEN] * (fp64 ? 8 : 4)));
        const colors = info[INFO_COLORS_LEN] > 0 ? new Uint8Array(copyFromWasm(wasm, info[INFO_COLORS_PTR], info[INFO_COLORS_LEN])) : null;
        const intensities = new Uint16Array(copyFromWasm(wasm, info[INFO_INTENSITIES_PTR], info[INFO_INTENSITIES_LEN] * 2));
        const classifications = new Uint8Array(copyFromWasm(wasm, info[INFO_CLASSIFICATIONS_PTR], info[INFO_CLASSIFICATIONS_LEN]));
        const header = JSON.parse(readWasmString(wasm, info[INFO_HEADER_PTR], info[INFO_HEADER_LEN]));
        return {header, pointsFormatId: info[INFO_POINT_FORMAT], numPoints: info[INFO_NUM_KEPT], positions, colors, intensities, classifications};
    } finally {
        exports.las_close(reader);
    }
}
