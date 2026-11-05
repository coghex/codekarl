#version 450
// The map pass's vertex stage (milestone 6, slice 1): one triangle that covers the whole map target, from the vertex index, with no
// vertex buffer. (A triangle list of three vertices, so MoltenVK has no primitive restart to warn about.) The vertices are
// (-1, -1), (3, -1) and (-1, 3) in clip space: the clip square is inside the triangle, and every pixel of the target is shaded once.

void main() {
    vec2 corner = vec2(float((gl_VertexIndex << 1) & 2), float(gl_VertexIndex & 2));
    gl_Position = vec4(corner * 2.0 - 1.0, 0.0, 1.0);
}
