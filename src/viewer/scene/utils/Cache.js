/**
 * @typedef {Object} CacheType
 * @property {Boolean} enabled
 * @property {Object.<string, *>} files
 * @property {function(string, *): void} add
 * @property {function(string): *} get
 * @property {function(string): void} remove
 * @property {function(): void} clear
 */

/** @type {CacheType} */
const Cache = {

    enabled: false,
    files: {},

    add: function (key, file) {
        if (this.enabled === false) {
            return;
        }
        this.files[key] = file;
    },

    get: function (key) {
        if (this.enabled === false) {
            return;
        }
        return this.files[key];
    },

    remove: function (key) {
        delete this.files[key];
    },

    clear: function () {
        this.files = {};
    }
};

export {Cache};
