#version 450
// The scene's fragment stage: the sampled texel as it is. The image is premultiplied RGBA8 and the pipeline blends
// with ONE, ONE_MINUS_SRC_ALPHA, so nothing is multiplied here. The sampler (set 0) and the image (set 1) are
// separate descriptors: a filter change rebinds one small set and rewrites nothing.

layout(set = 0, binding = 0) uniform sampler scene_sampler;
layout(set = 1, binding = 0) uniform texture2D scene_image;

layout(location = 0) in vec2 coordinate;
layout(location = 0) out vec4 colour;

void main() {
    colour = texture(sampler2D(scene_image, scene_sampler), coordinate);
}
