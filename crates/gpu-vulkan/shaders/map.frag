#version 450
#extension GL_EXT_samplerless_texture_functions : require
// The map pass's fragment stage (milestone 6, slice 1): the minimal colour rule. Map pixel (x, y) shows the palette colour of grid A's
// terrain index at cell (left + x - i0, top + y - j0), one cell per map pixel, and palette entry 0 outside the crop. Everything is an
// integer read (`texelFetch`, never a filtered sample): grid A is an RGBA8 image of the crop's size (R the terrain index) and the
// palette a 256 x 1 RGBA8 image. A unorm byte read as a float and scaled by 255 rounds back to the original integer, so the result is
// the same on every GPU and on Metal. `gl_FragCoord` has its origin at the target's top-left corner and a half at the pixel centre,
// so its integer part is the pixel's column and row.

layout(push_constant) uniform Camera {
    ivec2 origin;   // the crop's global origin cell (i0, j0)
    ivec2 crop;     // the crop's size in cells
    ivec2 top_left; // the camera's top-left world pixel (I_left, J_top)
} camera;

layout(set = 0, binding = 0) uniform texture2D grid_a;
layout(set = 0, binding = 2) uniform texture2D palette;

layout(location = 0) out vec4 colour;

void main() {
    ivec2 pixel = ivec2(gl_FragCoord.xy);
    ivec2 cell = camera.top_left + pixel - camera.origin;
    uint index = 0u;
    if (all(greaterThanEqual(cell, ivec2(0))) && all(lessThan(cell, camera.crop))) {
        index = uint(texelFetch(grid_a, cell, 0).r * 255.0 + 0.5);
    }
    colour = texelFetch(palette, ivec2(int(index), 0), 0);
}
