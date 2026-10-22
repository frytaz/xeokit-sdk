/**
 * xeokit runtime statistics.
 * @type {{build: {version: string}, client: {browser: string}, components: {scenes: number, models: number, meshes: number, objects: number}, memory: {meshes: number, positions: number, colors: number, normals: number, uvs: number, indices: number, textures: number, transforms: number, materials: number, programs: number}, frame: {frameCount: number, fps: number, useProgram: number, bindTexture: number, bindArray: number, drawElements: number, drawArrays: number, tasksRun: number, tasksScheduled: number}}}
 */
const stats = {
    build: {
        version: "0.8"
    },
    client: {
        browser: (navigator && navigator.userAgent) ? navigator.userAgent : "n/a"
    },

    components: {
        scenes: 0,
        models: 0,
        meshes: 0,
        objects: 0
    },
    memory: {
        meshes: 0,
        positions: 0,
        colors: 0,
        normals: 0,
        uvs: 0,
        indices: 0,
        textures: 0,
        transforms: 0,
        materials: 0,
        programs: 0
    },
    frame: {
        frameCount: 0,
        fps: 0,
        useProgram: 0,
        bindTexture: 0,
        bindArray: 0,
        drawElements: 0,
        drawArrays: 0,
        tasksRun: 0,
        tasksScheduled: 0
    }
};

export {stats};