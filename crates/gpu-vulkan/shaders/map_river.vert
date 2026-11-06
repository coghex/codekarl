#version 450
// One instanced quad covers one prepared river pixel. Coordinates are view-relative and the camera's crop slot carries the view size.
layout(location = 0) in uvec2 river_pixel;

layout(push_constant) uniform RiverCamera {
    ivec2 origin;
    ivec2 view;
    ivec2 top_left;
    int zoom;
    int unused;
} camera;

void main() {
    const vec2 corners[6] = vec2[6](
        vec2(0.0, 0.0), vec2(0.0, 1.0), vec2(1.0, 0.0),
        vec2(1.0, 0.0), vec2(0.0, 1.0), vec2(1.0, 1.0));
    vec2 pixel = vec2(river_pixel) + corners[gl_VertexIndex];
    // With a positive-height Vulkan viewport, NDC -1 maps to the first (top) row.
    vec2 clip = vec2(2.0 * pixel.x / float(camera.view.x) - 1.0,
                     2.0 * pixel.y / float(camera.view.y) - 1.0);
    gl_Position = vec4(clip, 0.0, 1.0);
}
