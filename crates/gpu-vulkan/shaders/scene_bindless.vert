#version 450
// The scene's vertex stage through the bindless table (milestone 5.5, issue 6): `scene.vert` with the instance's native slot, a
// third instance-rate attribute (a `R32_UINT` vertex-buffer format every Vulkan device supports) passed to the fragment stage as a
// `flat` varying, so one draw samples a different table entry for each instance. The destination and visible source, the corners,
// the pixel-to-clip conversion and the texture-coordinate mapping are `scene.vert`'s; the push constant is the drawable's size.

layout(push_constant) uniform Target {
    vec2 size;
} target;

layout(location = 0) in vec4 destination;
layout(location = 1) in vec4 visible;
layout(location = 2) in uint slot;

layout(location = 0) out vec2 coordinate;
layout(location = 1) flat out uint table_slot;

void main() {
    // The corners of the two triangles (0, 1, 2) and (2, 1, 3): 0 = top-left, 1 = top-right, 2 = bottom-left,
    // 3 = bottom-right.
    int corner = gl_VertexIndex == 3 ? 2 : (gl_VertexIndex == 4 ? 1 : (gl_VertexIndex == 5 ? 3 : gl_VertexIndex));
    bool right = (corner & 1) != 0;
    bool bottom = (corner & 2) != 0;
    vec2 pixel = vec2(right ? destination.z : destination.x, bottom ? destination.w : destination.y);
    coordinate = vec2(right ? visible.z : visible.x, bottom ? visible.w : visible.y);
    table_slot = slot;
    gl_Position = vec4(2.0 * pixel / target.size - 1.0, 0.0, 1.0);
}
