#version 450
// The scene's vertex stage: one instance-rate vertex binding carries each quad's destination (in drawable pixels,
// left, top, right, bottom) and the visible part of its texture (as fractions of the image, left, top, right,
// bottom); six vertices of a triangle list (two triangles) make the quad. The push constant is the drawable's size in
// pixels. (A strip would be four vertices, but MoltenVK warns that Metal cannot disable primitive restart for one; the
// list is the same pixels and an empty validation log.)
// Corners, the pixel-to-clip conversion and the texture-coordinate mapping are `quad.vert`'s: Vulkan's +y points down
// the framebuffer, so a top-left pixel origin needs no flip, and v grows down the image, whose first row is the top.

layout(push_constant) uniform Target {
    vec2 size;
} target;

layout(location = 0) in vec4 destination;
layout(location = 1) in vec4 visible;

layout(location = 0) out vec2 coordinate;

void main() {
    // The corners of the two triangles (0, 1, 2) and (2, 1, 3): 0 = top-left, 1 = top-right, 2 = bottom-left,
    // 3 = bottom-right.
    int corner = gl_VertexIndex == 3 ? 2 : (gl_VertexIndex == 4 ? 1 : (gl_VertexIndex == 5 ? 3 : gl_VertexIndex));
    bool right = (corner & 1) != 0;
    bool bottom = (corner & 2) != 0;
    vec2 pixel = vec2(right ? destination.z : destination.x, bottom ? destination.w : destination.y);
    coordinate = vec2(right ? visible.z : visible.x, bottom ? visible.w : visible.y);
    gl_Position = vec4(2.0 * pixel / target.size - 1.0, 0.0, 1.0);
}
