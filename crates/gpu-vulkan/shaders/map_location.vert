#version 450
layout(location = 0) in uvec3 location_pixel;
layout(location = 0) flat out uint palette_index;
layout(push_constant) uniform LocationCamera {
    ivec2 unused_a;
    ivec2 view;
    ivec2 unused_b;
    int unused_c;
    int unused_d;
} camera;
void main() {
    const vec2 corners[6] = vec2[6](
        vec2(0.0, 0.0), vec2(0.0, 1.0), vec2(1.0, 0.0),
        vec2(1.0, 0.0), vec2(0.0, 1.0), vec2(1.0, 1.0));
    vec2 pixel = vec2(location_pixel.xy) + corners[gl_VertexIndex];
    gl_Position = vec4(2.0 * pixel.x / float(camera.view.x) - 1.0,
                       2.0 * pixel.y / float(camera.view.y) - 1.0, 0.0, 1.0);
    palette_index = location_pixel.z;
}
