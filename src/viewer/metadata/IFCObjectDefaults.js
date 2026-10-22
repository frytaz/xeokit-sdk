/**
 * Map of IFC type names to default initial properties for {@link Entity}s of that type.
 *
 * @typedef {{[key: string]: {colorize?: number[], pickable?: boolean, visible?: boolean, opacity?: number}}} IFCObjectDefaults
 */

/**
 * @desc Default initial properties for {@link Entity}s loaded from models accompanied by metadata.
 *
 * When loading a model, plugins such as {@link XKTLoaderPlugin} create
 * a tree of {@link Entity}s that represent the model. These loaders can optionally load metadata, to create
 * a {@link MetaModel} corresponding to the root {@link Entity}, with a {@link MetaObject} corresponding to each
 * object {@link Entity} within the tree.
 *
 * @type {IFCObjectDefaults}
 */
const IFCObjectDefaults = {

    DEFAULT: {
    }
};

export {IFCObjectDefaults}