#version 450
layout(location = 0) flat in uint palette_index;
layout(set = 0, binding = 3, std430) readonly buffer LocationPalette { uint colours[]; } palette;
layout(location = 0) out vec4 colour;
void main() {
    uint packed = palette.colours[palette_index];
    colour = vec4(float(packed & 0xffu), float((packed >> 8u) & 0xffu),
                  float((packed >> 16u) & 0xffu), float((packed >> 24u) & 0xffu)) / 255.0;
}
