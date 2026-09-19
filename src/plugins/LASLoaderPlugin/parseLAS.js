import {lasWasm} from "./lasWasm.js";
import {compileWasm, loadWasm} from "../lib/wasm/loadWasm.js";

/**
 * Parses a LAS or LAZ (LASzip-compressed) file with the WebAssembly decoder built from wasm/las.
 *
 * Supports LAS 1.0 - 1.5, point data record formats 0 - 10, with or without LASzip compression.
 *
 * Decoding runs in a Web Worker when the browser allows one (created from a Blob URL, so no separate script has
 * to be hosted); otherwise, or when `options.worker` is false, it runs on the calling thread, yielding to the
 * event loop between chunks of points.
 *
 * @private
 * @param {ArrayBuffer} arrayBuffer The LAS/LAZ file.
 * @param {Object} [options]
 * @param {Number} [options.skip=1] Load every n-th point.
 * @param {Boolean} [options.fp64=false] Return positions as Float64Array instead of Float32Array.
 * @param {Number|String} [options.colorDepth="auto"] 8, 16 or "auto" - whether colors are stored in 8 or 16 bits.
 * @param {Boolean} [options.worker=true] Decode in a Web Worker when available.
 * @param {Boolean} [options.transfer=false] Allow `arrayBuffer` to be transferred to the worker (it is then unusable
 * to the caller afterwards); otherwise the worker gets a copy.
 * @returns {Promise<{header: Object, pointsFormatId: Number, numPoints: Number, positions: Float32Array|Float64Array, colors: Uint8Array|null, intensities: Uint16Array, classifications: Uint8Array}>}
 * `colors` holds 8-bit RGB triples, `intensities` the raw 16-bit intensities, `header` the public header block
 * (with the LAS 1.5 `MaxGPSTime`, `MinGPSTime` and `TimeOffset` fields when present), the `CoordinateSystemWKT`
 * and the `epsg` code from the GeoTIFF keys or the WKT record, when found.
 */
export async function parseLAS(arrayBuffer, options = {}) {
    const decodeOptions = {skip: options.skip, fp64: options.fp64, colorDepth: options.colorDepth};
    if (options.worker !== false && workerSupported()) {
        try {
            return finish(await decodeInWorker(arrayBuffer, decodeOptions, !!options.transfer));
        } catch (e) {
            if (!e || !e.workerUnavailable) {
                throw e;
            }
        }
    }
    const wasm = await loadWasm(lasWasm);
    return finish(await decodeLAS(wasm.exports, new Uint8Array(arrayBuffer), decodeOptions, true));
}

function finish(result) {
    result.header = JSON.parse(result.header);
    return result;
}

/**
 * Decodes a LAS/LAZ file with an instantiated wasm/las module. Self-contained on purpose - its source is also
 * injected into the Web Worker through Function.prototype.toString, so it must not reference anything outside
 * itself, and it must not use syntax that the ES5 build would compile into shared helper functions.
 *
 * @param exports The module exports.
 * @param {Uint8Array} bytes The file.
 * @param options skip, fp64, colorDepth.
 * @param {Boolean} yieldBetweenChunks Pause on a macrotask every ~50ms so the calling thread stays responsive.
 * @returns {Promise<Object>} The decoded arrays and the header as a JSON string.
 */
function decodeLAS(exports, bytes, options, yieldBetweenChunks) {
    return new Promise(function (resolve, reject) {
        var lastError = function () {
            return new TextDecoder().decode(new Uint8Array(exports.memory.buffer, exports.error_ptr(), exports.error_len()));
        };
        var ptr = exports.alloc(bytes.length);
        new Uint8Array(exports.memory.buffer, ptr, bytes.length).set(bytes);
        var colorDepth = (options.colorDepth === 8 || options.colorDepth === 16) ? options.colorDepth : 0;
        var reader = exports.las_open(ptr, bytes.length, Math.max(1, Math.floor(options.skip || 1)), options.fp64 ? 1 : 0, colorDepth);
        if (!reader) {
            reject(new Error(lastError()));
            return;
        }
        var lastYield = Date.now();
        var step = function () {
            try {
                for (; ;) {
                    var remaining = exports.las_read(reader, 200000);
                    if (remaining === 0xFFFFFFFF) {
                        throw new Error(lastError());
                    }
                    if (remaining === 0) {
                        break;
                    }
                    if (yieldBetweenChunks && Date.now() - lastYield > 50) {
                        lastYield = Date.now();
                        setTimeout(step, 0);
                        return;
                    }
                }
                // Info struct written by the module (see wasm/las/src/lib.rs, `Info`): 15 little-endian u32 fields
                var info = new Uint32Array(exports.memory.buffer, exports.las_info(reader), 15);
                var fp64 = info[3] !== 0;
                var copy = function (p, len) {
                    return exports.memory.buffer.slice(p, p + len);
                };
                var result = {
                    header: new TextDecoder().decode(new Uint8Array(exports.memory.buffer, info[12], info[13])),
                    pointsFormatId: info[2],
                    numPoints: info[0],
                    positions: new (fp64 ? Float64Array : Float32Array)(copy(info[4], info[5] * (fp64 ? 8 : 4))),
                    colors: info[7] > 0 ? new Uint8Array(copy(info[6], info[7])) : null,
                    intensities: new Uint16Array(copy(info[8], info[9] * 2)),
                    classifications: new Uint8Array(copy(info[10], info[11]))
                };
                exports.las_close(reader);
                resolve(result);
            } catch (e) {
                exports.las_close(reader);
                reject(e);
            }
        };
        step();
    });
}

/**
 * Body of the Web Worker: instantiates the module it is sent, then decodes each file it is sent. Same constraints
 * as decodeLAS, which it calls by name (both sources are concatenated into the worker script).
 */
function lasWorkerMain() {
    var instancePromise = null;
    self.onmessage = function (event) {
        var msg = event.data;
        if (msg.type === "init") {
            instancePromise = WebAssembly.instantiate(msg.module, {});
            instancePromise.catch(function (e) {
                self.postMessage({type: "error", error: String(e && e.message || e)});
            });
        } else if (msg.type === "decode") {
            instancePromise.then(function (instance) {
                return decodeLAS(instance.exports, new Uint8Array(msg.buffer), msg.options, false);
            }).then(function (result) {
                var transfer = [result.positions.buffer, result.intensities.buffer, result.classifications.buffer];
                if (result.colors) {
                    transfer.push(result.colors.buffer);
                }
                self.postMessage({type: "result", id: msg.id, result: result}, transfer);
            }, function (e) {
                self.postMessage({type: "result", id: msg.id, error: String(e && e.message || e)});
            });
        }
    };
}

const WORKER_IDLE_TIMEOUT = 10000; // Terminate the worker (and free its wasm memory) after this idle time

const workerState = {
    url: null,
    worker: null,
    pending: new Map(),
    nextId: 1,
    unavailable: false,
    idleTimer: null
};

function workerSupported() {
    return !workerState.unavailable && typeof Worker !== "undefined" && typeof Blob !== "undefined" && typeof URL !== "undefined" && !!URL.createObjectURL;
}

function unavailableError(cause) {
    const error = new Error(`LAS worker unavailable: ${cause && cause.message ? cause.message : cause}`);
    error.workerUnavailable = true;
    return error;
}

/** Rejects every pending decode; when the worker itself failed, callers fall back to the calling thread. */
function failPending(cause, workerUnavailable) {
    for (const pending of workerState.pending.values()) {
        pending.reject(workerUnavailable ? unavailableError(cause) : new Error(cause));
    }
    workerState.pending.clear();
    if (workerUnavailable) {
        workerState.unavailable = true;
        terminateWorker();
    }
}

function terminateWorker() {
    if (workerState.worker) {
        workerState.worker.terminate();
        workerState.worker = null;
    }
    clearTimeout(workerState.idleTimer);
    workerState.idleTimer = null;
}

async function getWorker() {
    const module = await compileWasm(lasWasm);
    if (!workerState.worker) {
        try {
            if (!workerState.url) {
                const source = [decodeLAS.toString(), "(" + lasWorkerMain.toString() + ")();"].join("\n");
                workerState.url = URL.createObjectURL(new Blob([source], {type: "text/javascript"}));
            }
            const worker = new Worker(workerState.url);
            worker.onmessage = (event) => {
                const msg = event.data;
                if (msg.type === "result") {
                    const pending = workerState.pending.get(msg.id);
                    workerState.pending.delete(msg.id);
                    if (pending) {
                        msg.error !== undefined ? pending.reject(new Error(msg.error)) : pending.resolve(msg.result);
                    }
                    if (workerState.pending.size === 0) {
                        clearTimeout(workerState.idleTimer);
                        workerState.idleTimer = setTimeout(terminateWorker, WORKER_IDLE_TIMEOUT);
                    }
                } else if (msg.type === "error") {
                    failPending(msg.error, true);
                }
            };
            worker.onerror = (event) => {
                failPending(event.message || "worker error", true);
            };
            worker.postMessage({type: "init", module});
            workerState.worker = worker;
        } catch (e) { // e.g. Content-Security-Policy forbids blob: workers, or the module cannot be cloned
            workerState.unavailable = true;
            terminateWorker();
            throw unavailableError(e);
        }
    }
    clearTimeout(workerState.idleTimer);
    return workerState.worker;
}

function decodeInWorker(arrayBuffer, options, transfer) {
    return getWorker().then((worker) => new Promise((resolve, reject) => {
        const id = workerState.nextId++;
        workerState.pending.set(id, {resolve, reject});
        try {
            const buffer = transfer ? arrayBuffer : arrayBuffer.slice(0);
            worker.postMessage({type: "decode", id, buffer, options}, [buffer]);
        } catch (e) {
            workerState.pending.delete(id);
            reject(unavailableError(e));
        }
    }));
}
