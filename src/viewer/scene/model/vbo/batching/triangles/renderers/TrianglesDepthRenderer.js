import {TrianglesBatchingRenderer} from "./TrianglesBatchingRenderer.js";


/**
 * @private
 */
export class TrianglesDepthRenderer extends TrianglesBatchingRenderer {

    _buildVertexShader() {
        const scene = this._scene;
        const clipping = scene._sectionPlanesState.getNumAllocatedSectionPlanes() > 0;
        const src = [];
        src.push("#version 300 es");
        src.push("// Triangles batching depth vertex shader");

        src.push("uniform int renderPass;");
        src.push("in vec3 position;");
        if (scene.entityOffsetsEnabled) {
            src.push("in vec3 offset;");
        }
        src.push("in float flags;");

        this._addMatricesUniformBlockLines(src);

        if (scene.logarithmicDepthBufferEnabled) {
            src.push("uniform float logDepthBufFC;");
            src.push("out float vFragDepth;");
            src.push("bool isPerspectiveMatrix(mat4 m) {");
            src.push("    return (m[2][3] == - 1.0);");
            src.push("}");
            src.push("out float isPerspective;");
        }
        if (clipping) {
            src.push("out vec4 vWorldPosition;");
            src.push("out float vFlags;");
        }
        src.push("out vec2 vHighPrecisionZW;");
        src.push("void main(void) {");

        // colorFlag = NOT_RENDERED | COLOR_OPAQUE | COLOR_TRANSPARENT
        // renderPass = COLOR_OPAQUE

        src.push(`int colorFlag = int(flags) & 0xF;`);
        src.push(`if (colorFlag != renderPass) {`);
        src.push("      gl_Position = vec4(0.0, 0.0, 0.0, 0.0);"); // Cull vertex
        src.push("  } else {");
        src.push("      vec4 worldPosition = worldMatrix * (positionsDecodeMatrix * vec4(position, 1.0)); ");
        if (scene.entityOffsetsEnabled) {
            src.push("      worldPosition.xyz = worldPosition.xyz + offset;");
        }
        src.push("      vec4 viewPosition  = viewMatrix * worldPosition; ");
        if (clipping) {
            src.push("      vWorldPosition = worldPosition;");
            src.push("      vFlags = flags;");
        }
        src.push("vec4 clipPos = projMatrix * viewPosition;");
        if (scene.logarithmicDepthBufferEnabled) {
            src.push("vFragDepth = 1.0 + clipPos.w;");
            src.push("isPerspective = float (isPerspectiveMatrix(projMatrix));");
        }
        src.push("gl_Position = clipPos;");
        src.push("vHighPrecisionZW = gl_Position.zw;");
        src.push("  }");
        src.push("}");
        return src;
    }

    _buildFragmentShader() {
        const scene = this._scene;
        const sectionPlanesState = scene._sectionPlanesState;
        const clipping = (sectionPlanesState.getNumAllocatedSectionPlanes() > 0);
        const src = [];
        src.push("#version 300 es");
        src.push("// Triangles batching depth fragment shader");

        src.push("precision highp float;");
        src.push("precision highp int;");
        if (scene.logarithmicDepthBufferEnabled) {
            src.push("in float isPerspective;");
            src.push("uniform float logDepthBufFC;");
            src.push("in float vFragDepth;");
        }
        if (clipping) {
            src.push("in vec4 vWorldPosition;");
            src.push("in float vFlags;");
            for (let i = 0; i < sectionPlanesState.getNumAllocatedSectionPlanes(); i++) {
                src.push("uniform bool sectionPlaneActive" + i + ";");
                src.push("uniform vec3 sectionPlanePos" + i + ";");
                src.push("uniform vec3 sectionPlaneDir" + i + ";");
            }
        }
        src.push("in vec2 vHighPrecisionZW;");
        src.push("out vec4 outColor;");
        src.push("void main(void) {");
        if (clipping) {
            src.push("  bool clippable = (int(vFlags) >> 16 & 0xF) == 1;");
            src.push("  if (clippable) {");
            src.push("      float dist = 0.0;");
            for (var i = 0; i < sectionPlanesState.getNumAllocatedSectionPlanes(); i++) {
                src.push("      if (sectionPlaneActive" + i + ") {");
                src.push("          dist += clamp(dot(-sectionPlaneDir" + i + ".xyz, vWorldPosition.xyz - sectionPlanePos" + i + ".xyz), 0.0, 1000.0);");
                src.push("      }");
            }
            src.push("      if (dist > 0.0) { discard; }");
            src.push("  }");
        }
        if (scene.logarithmicDepthBufferEnabled) {
            src.push("    gl_FragDepth = isPerspective == 0.0 ? gl_FragCoord.z : log2( vFragDepth ) * logDepthBufFC * 0.5;");
        }
        src.push("float fragCoordZ = 0.5 * vHighPrecisionZW[0] / vHighPrecisionZW[1] + 0.5;");
        src.push("    outColor = vec4(vec3(1.0 - fragCoordZ), 1.0); ");
        src.push("}");
        return src;
    }
}
