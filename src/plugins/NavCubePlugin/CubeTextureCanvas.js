import {math} from "../../viewer/scene/math/math.js";

const swizzleYUp = {
    "NavCube.front": "NavCube.front",
    "NavCube.back": "NavCube.back",
    "NavCube.right": "NavCube.right",
    "NavCube.left": "NavCube.left",
    "NavCube.top": "NavCube.top",
    "NavCube.bottom": "NavCube.bottom"
};

const swizzleZUp = {
    "NavCube.front": "NavCube.front",
    "NavCube.back": "NavCube.back",
    "NavCube.right": "NavCube.right",
    "NavCube.left": "NavCube.left",
    "NavCube.top": "NavCube.top",
    "NavCube.bottom": "NavCube.bottom"
};

const defaultLabels = {
    "NavCube.front": "FRONT",
    "NavCube.back": "BACK",
    "NavCube.right": "RIGHT",
    "NavCube.left": "LEFT",
    "NavCube.top": "TOP",
    "NavCube.bottom": "BOTTOM"
};

/**
 * @private
 */
class CubeTextureCanvas {

    #viewer;
    #navCubeScene;
    #cubeColor;
    #cubeHighlightColor;
    #textColor;
    #width;
    #height;
    #scale;
    #facesZUp;
    #areasZUp;
    #facesYUp;
    #areasYUp;
    #faces;
    #areas;
    #context;
    #zUp = false;

    constructor(viewer, navCubeScene, cfg = {}) {

        const cubeColor = "lightgrey";
        const cubeHighlightColor = cfg.hoverColor || "rgba(0,0,0,0.4)";
        const textColor = cfg.textColor || "black";
        const parentNode = cfg.canvasElement || document.body;

        const height = 500;
        const width = height + (height / 3);
        const scale = width / 24;

        const facesZUp = [
            {boundary: [6, 6, 6, 6], color: cfg.frontColor || cfg.color || "#55FF55"},
            {boundary: [18, 6, 6, 6], color: cfg.backColor || cfg.color || "#55FF55"},
            {boundary: [12, 6, 6, 6], color: cfg.rightColor || cfg.color || "#FF5555"},
            {boundary: [0, 6, 6, 6], color: cfg.leftColor || cfg.color || "#FF5555"},
            {boundary: [6, 0, 6, 6], color: cfg.topColor || cfg.color || "#7777FF"},
            {boundary: [6, 12, 6, 6], color: cfg.bottomColor || cfg.color || "#7777FF"}
        ];

        const areasZUp = [
            {label: "NavCube.front", boundaries: [[7, 7, 4, 4]], dir: [0, 1, 0], up: [0, 0, 1]},
            {label: "NavCube.back", boundaries: [[19, 7, 4, 4]], dir: [0, -1, 0], up: [0, 0, 1]},
            {label: "NavCube.right", boundaries: [[13, 7, 4, 4]], dir: [-1, 0, 0], up: [0, 0, 1]},
            {label: "NavCube.left", boundaries: [[1, 7, 4, 4]], dir: [1, 0, 0], up: [0, 0, 1]},
            {label: "NavCube.top", boundaries: [[7, 1, 4, 4]], dir: [0, 0, -1], up: [0, 1, 0]},
            {label: "NavCube.bottom", boundaries: [[7, 13, 4, 4]], dir: [0, 0, 1], up: [0, -1, 0]},
            {boundaries: [[7, 5, 4, 2]], dir: [0, 1, -1], up: [0, 1, 1]},
            {boundaries: [[1, 6, 4, 1], [6, 1, 1, 4]], dir: [1, 0, -1], up: [1, 0, 1]},
            {boundaries: [[7, 0, 4, 1], [19, 6, 4, 1]], dir: [0, -1, -1], up: [0, -1, 1]},
            {boundaries: [[13, 6, 4, 1], [11, 1, 1, 4]], dir: [-1, 0, -1], up: [-1, 0, 1]},
            {boundaries: [[7, 11, 4, 2]], dir: [0, 1, 1], up: [0, -1, 1]},
            {boundaries: [[1, 11, 4, 1], [6, 13, 1, 4]], dir: [1, 0, 1], up: [-1, 0, 1]},
            {boundaries: [[7, 17, 4, 1], [19, 11, 4, 1]], dir: [0, -1, 1], up: [0, 1, 1]},
            {boundaries: [[13, 11, 4, 1], [11, 13, 1, 4]], dir: [-1, 0, 1], up: [1, 0, 1]},
            {boundaries: [[5, 7, 2, 4]], dir: [1, 1, 0], up: [0, 0, 1]},
            {boundaries: [[11, 7, 2, 4]], dir: [-1, 1, 0], up: [0, 0, 1]},
            {boundaries: [[17, 7, 2, 4]], dir: [-1, -1, 0], up: [0, 0, 1]},
            {boundaries: [[0, 7, 1, 4], [23, 7, 1, 4]], dir: [1, -1, 0], up: [0, 0, 1]},
            {boundaries: [[5, 11, 2, 2]], dir: [1, 1, 1], up: [-1, -1, 1]},
            {boundaries: [[23, 11, 1, 1], [6, 17, 1, 1], [0, 11, 1, 1]], dir: [1, -1, 1], up: [-1, 1, 1]},
            {boundaries: [[5, 5, 2, 2]], dir: [1, 1, -1], up: [1, 1, 1]},
            {boundaries: [[11, 17, 1, 1], [17, 11, 2, 1]], dir: [-1, -1, 1], up: [1, 1, 1]},
            {boundaries: [[17, 6, 2, 1], [11, 0, 1, 1]], dir: [-1, -1, -1], up: [-1, -1, 1]},
            {boundaries: [[11, 11, 2, 2]], dir: [-1, 1, 1], up: [1, -1, 1]},
            {boundaries: [[0, 6, 1, 1], [6, 0, 1, 1], [23, 6, 1, 1]], dir: [1, -1, -1], up: [1, -1, 1]},
            {boundaries: [[11, 5, 2, 2]], dir: [-1, 1, -1], up: [-1, 1, 1]}
        ];

        const facesYUp = [
            {boundary: [6, 6, 6, 6], color: cfg.frontColor || cfg.color || "#55FF55"},
            {boundary: [18, 6, 6, 6], color: cfg.backColor || cfg.color || "#55FF55"},
            {boundary: [12, 6, 6, 6], color: cfg.rightColor || cfg.color || "#FF5555"},
            {boundary: [0, 6, 6, 6], color: cfg.leftColor || cfg.color || "#FF5555"},
            {boundary: [6, 0, 6, 6], color: cfg.topColor || cfg.color || "#7777FF"},
            {boundary: [6, 12, 6, 6], color: cfg.bottomColor || cfg.color || "#7777FF"}
        ];

        const areasYUp = [

            // Faces

            {yUp: "", label: "NavCube.front", boundaries: [[7, 7, 4, 4]], dir: [0, 0, -1], up: [0, 1, 0]},
            {label: "NavCube.back", boundaries: [[19, 7, 4, 4]], dir: [0, 0, 1], up: [0, 1, 0]},
            {label: "NavCube.right", boundaries: [[13, 7, 4, 4]], dir: [-1, 0, 0], up: [0, 1, 0]},
            {label: "NavCube.left", boundaries: [[1, 7, 4, 4]], dir: [1, 0, 0], up: [0, 1, 0]},
            {label: "NavCube.top", boundaries: [[7, 1, 4, 4]], dir: [0, -1, 0], up: [0, 0, -1]},
            {label: "NavCube.bottom", boundaries: [[7, 13, 4, 4]], dir: [0, 1, 0], up: [0, 0, 1]},
            {boundaries: [[7, 5, 4, 2]], dir: [0, -0.7071, -0.7071], up: [0, 0.7071, -0.7071]}, // Top-front edge
            {boundaries: [[1, 6, 4, 1], [6, 1, 1, 4]], dir: [1, -1, 0], up: [1, 1, 0]},  // Top-left edge
            {boundaries: [[7, 0, 4, 1], [19, 6, 4, 1]], dir: [0, -0.7071, 0.7071], up: [0, 0.7071, 0.7071]}, // Top-back edge
            {boundaries: [[13, 6, 4, 1], [11, 1, 1, 4]], dir: [-1, -1, 0], up: [-1, 1, 0]}, // Top-right edge
            {boundaries: [[7, 11, 4, 2]], dir: [0, 1, -1], up: [0, 1, 1]},  // Bottom-front edge
            {boundaries: [[1, 11, 4, 1], [6, 13, 1, 4]], dir: [1, 1, 0], up: [-1, 1, 0]}, // Bottom-left edge
            {boundaries: [[7, 17, 4, 1], [19, 11, 4, 1]], dir: [0, 1, 1], up: [0, 1, -1]}, // Bottom-back edge
            {boundaries: [[13, 11, 4, 1], [11, 13, 1, 4]], dir: [-1, 1, 0], up: [1, 1, 0]}, // Bottom-right edge
            {boundaries: [[5, 7, 2, 4]], dir: [1, 0, -1], up: [0, 1, 0]},// Front-left edge
            {boundaries: [[11, 7, 2, 4]], dir: [-1, 0, -1], up: [0, 1, 0]}, // Front-right edge
            {boundaries: [[17, 7, 2, 4]], dir: [-1, 0, 1], up: [0, 1, 0]},// Back-right edge
            {boundaries: [[0, 7, 1, 4], [23, 7, 1, 4]], dir: [1, 0, 1], up: [0, 1, 0]},// Back-left edge
            {boundaries: [[5, 11, 2, 2]], "dir": [0.5, 0.7071, -0.5], "up": [-0.5, 0.7071, 0.5]}, // Bottom-left-front corner
            {boundaries: [[23, 11, 1, 1], [6, 17, 1, 1], [0, 11, 1, 1]],"dir": [0.5, 0.7071, 0.5],"up": [-0.5, 0.7071, -0.5]},// Bottom-back-left corner
            {boundaries: [[5, 5, 2, 2]], "dir": [0.5, -0.7071, -0.5], "up": [0.5, 0.7071, -0.5]}, // Left-front-top corner
            {boundaries: [[11, 17, 1, 1], [17, 11, 2, 1]], "dir": [-0.5, 0.7071, 0.5], "up": [0.5, 0.7071, -0.5]}, // Bottom-back-right corner
            {boundaries: [[17, 6, 2, 1], [11, 0, 1, 1]], "dir": [-0.5, -0.7071, 0.5], "up": [-0.5, 0.7071, 0.5]}, // Top-back-right corner
            {boundaries: [[11, 11, 2, 2]], "dir": [-0.5, 0.7071, -0.5], "up": [0.5, 0.7071, 0.5]}, // Bottom-front-right corner
            {boundaries: [[0, 6, 1, 1], [6, 0, 1, 1], [23, 6, 1, 1]], "dir": [0.5, -0.7071, 0.5], "up": [0.5, 0.7071, 0.5]},// Top-back-left corner
            {boundaries: [[11, 5, 2, 2]], "dir": [-0.5, -0.7071, -0.5], "up": [-0.5, 0.7071, -0.5]}// Top-front-right corner
        ];

        for (let i = 0, len = areasZUp.length; i < len; i++) {
            math.normalizeVec3(areasZUp[i].dir, areasZUp[i].dir);
            math.normalizeVec3(areasZUp[i].up, areasZUp[i].up);
        }

        for (let i = 0, len = areasYUp.length; i < len; i++) {
            math.normalizeVec3(areasYUp[i].dir, areasYUp[i].dir);
            math.normalizeVec3(areasYUp[i].up, areasYUp[i].up);
        }

        this.#viewer = viewer;
        this.#navCubeScene = navCubeScene;
        this.#cubeColor = cubeColor;
        this.#cubeHighlightColor = cubeHighlightColor;
        this.#textColor = textColor;
        this.#width = width;
        this.#height = height;
        this.#scale = scale;
        this.#facesZUp = facesZUp;
        this.#areasZUp = areasZUp;
        this.#facesYUp = facesYUp;
        this.#areasYUp = areasYUp;
        this.#faces = facesYUp;
        this.#areas = areasYUp;

        this._textureCanvas = document.createElement('canvas');
        this._textureCanvas.width = width;
        this._textureCanvas.height = height;
        this._textureCanvas.style.width = width + "px";
        this._textureCanvas.style.height = height + "px";
        this._textureCanvas.style.padding = "0";
        this._textureCanvas.style.margin = "0";
        this._textureCanvas.style.top = "0";
        this._textureCanvas.style.background = cubeColor;
        this._textureCanvas.style.position = "absolute";
        this._textureCanvas.style.opacity = "1.0";
        this._textureCanvas.style.visibility = "hidden";
        this._textureCanvas.style["z-index"] = 2000000;

        parentNode.appendChild(this._textureCanvas);

        this.#context = this._textureCanvas.getContext("2d");
    }

    #paint() {

        const context = this.#context;
        const scale = this.#scale;
        const areas = this.#areas;

        for (let i = 0, len = this.#facesZUp.length; i < len; i++) {
            const face = this.#facesZUp[i];
            const boundary = face.boundary;
            const xmin = Math.round(boundary[0] * scale);
            const ymin = Math.round(boundary[1] * scale);
            const width = Math.round(boundary[2] * scale);
            const height = Math.round(boundary[3] * scale);
            context.fillStyle = face.color;
            context.fillRect(xmin, ymin, width, height);
        }

        for (let i = 0, len = areas.length; i < len; i++) {
            let xmin;
            let ymin;
            let width;
            let height;
            const area = areas[i];

            const boundaries = area.boundaries;
            for (var j = 0, lenj = boundaries.length; j < lenj; j++) {
                const boundary = boundaries[j];
                xmin = Math.round(boundary[0] * scale);
                ymin = Math.round(boundary[1] * scale);
                width = Math.round(boundary[2] * scale);
                height = Math.round(boundary[3] * scale);
                if (area.highlighted) {
                    context.fillStyle = area.highlighted ? this.#cubeHighlightColor : (area.color || this.#cubeColor);
                    context.fillRect(xmin, ymin, width, height);
                }
            }
            if (area.label) {
                context.fillStyle = this.#textColor;
                context.font = '60px sans-serif';
                context.textAlign = "center";
                var xcenter = xmin + (width * 0.5);
                var ycenter = ymin + (height * 0.7);
                context.fillText(this.#translateLabel(area.label), xcenter, ycenter, 80);
            }
        }

        this.#navCubeScene.glRedraw();
    }

    #translateLabel(key) {
        const swizzle = this.#zUp ? swizzleZUp : swizzleYUp;
        const swizzledKey = swizzle ? swizzle[key] : null;
        if (swizzledKey) {
            return this.#viewer.localeService.translate(swizzledKey) || defaultLabels[swizzledKey] || swizzledKey;
        }
        return key;
    }

    setZUp() {
        this.#zUp = true;
        this.#faces = this.#facesZUp;
        this.#areas = this.#areasZUp;
        this.clear();
    }

    setYUp() {
        this.#zUp = false;
        this.#faces = this.#facesYUp;
        this.#areas = this.#areasYUp;
        this.clear();
    }

    clear() {
        const context = this.#context;
        const areas = this.#areas;
        context.fillStyle = this.#cubeColor;
        context.fillRect(0, 0, this.#width, this.#height);
        for (var i = 0, len = areas.length; i < len; i++) {
            const area = areas[i];
            area.highlighted = false;
        }
        this.#paint();
    }

    getArea(uv) {
        const scale = this.#scale;
        const areas = this.#areas;
        const s = uv[0] * this.#width;
        const t = this.#height - (uv[1] * this.#height); // Correct for our texture Y-flipping
        for (var i = 0, len = areas.length; i < len; i++) {
            const area = areas[i];
            const boundaries = area.boundaries;
            for (var j = 0, lenj = boundaries.length; j < lenj; j++) {
                const boundary = boundaries[j];
                if (s >= (boundary[0] * scale) && s <= ((boundary[0] + boundary[2]) * scale) && t >= (boundary[1] * scale) && t <= ((boundary[1] + boundary[3]) * scale)) {
                    return i;
                }
            }
        }
        return -1;
    }

    setAreaHighlighted(areaId, highlighted) {
        var area = this.#areas[areaId];
        if (!area) {
            throw "Area not found: " + areaId;
        }
        area.highlighted = !!highlighted;
        this.#paint();
    }

    getAreaDir(areaId) {
        var area = this.#areas[areaId];
        if (!area) {
            throw "Unknown area: " + areaId;
        }
        return area.dir;
    }

    getAreaUp(areaId) {
        var area = this.#areas[areaId];
        if (!area) {
            throw "Unknown area: " + areaId;
        }
        return area.up;
    }

    getImage() {
        return this._textureCanvas;
    }

    /**
     * @private
     */
    destroy() {
        if (this._textureCanvas) {
            this._textureCanvas.parentNode.removeChild(this._textureCanvas);
            this._textureCanvas = null;
        }
    }
}

export {CubeTextureCanvas};
