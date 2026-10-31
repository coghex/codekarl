#version 450
// The textured quad's fragment stage: the sampled texel as it is. The image is premultiplied RGBA8 and the
// pipeline blends with ONE, ONE_MINUS_SRC_ALPHA, so nothing is multiplied here.

layout(set = 0, binding = 0) uniform sampler2D image;

layout(location = 0) in vec2 coordinate;
layout(location = 0) out vec4 colour;

void main() {
    colour = texture(image, coordinate);
}
