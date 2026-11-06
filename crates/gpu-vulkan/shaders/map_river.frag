#version 450
#extension GL_EXT_samplerless_texture_functions : require

layout(set = 0, binding = 2) uniform texture2D rules;
layout(location = 0) out vec4 colour;

void main() {
    colour = texelFetch(rules, ivec2(3, 256), 0);
}
