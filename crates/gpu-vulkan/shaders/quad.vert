#version 450
// The textured quad's vertex stage: six vertices (two triangles) from two rectangles in push constants, with no
// vertex buffer. (A triangle list, not a strip: MoltenVK warns that Metal cannot disable primitive restart, and the
// validation layer's log must stay empty.) `destination` is the quad in normalised device coordinates (left, top, right, bottom;
// Vulkan's +y points down the framebuffer, so a top-left pixel origin needs no flip), `texture_rectangle` the
// part of the image to draw in texture coordinates (left, top, right, bottom; v grows down the image, whose
// first row is the top).

layout(push_constant) uniform Rectangles {
    vec4 destination;
    vec4 texture_rectangle;
} rectangles;

layout(location = 0) out vec2 coordinate;

void main() {
    // The corners of the two triangles (top-left, top-right, bottom-left) and (bottom-left, top-right,
    // bottom-right), as the corner numbers 0 = top-left, 1 = top-right, 2 = bottom-left, 3 = bottom-right.
    const int corners[6] = int[](0, 1, 2, 2, 1, 3);
    int corner = corners[gl_VertexIndex];
    bool right = (corner & 1) != 0;
    bool bottom = (corner & 2) != 0;
    vec2 position = vec2(
        right ? rectangles.destination.z : rectangles.destination.x,
        bottom ? rectangles.destination.w : rectangles.destination.y);
    coordinate = vec2(
        right ? rectangles.texture_rectangle.z : rectangles.texture_rectangle.x,
        bottom ? rectangles.texture_rectangle.w : rectangles.texture_rectangle.y);
    gl_Position = vec4(position, 0.0, 1.0);
}
