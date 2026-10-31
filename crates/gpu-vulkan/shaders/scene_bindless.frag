#version 450
#extension GL_EXT_nonuniform_qualifier : require
// The scene's fragment stage through the bindless table (milestone 5.5, issue 5): the sampled texel as it is, from the table's
// entry for the batch's slot. The image is premultiplied RGBA8 and the pipeline blends with ONE, ONE_MINUS_SRC_ALPHA, so
// nothing is multiplied here. The sampler (set 0) and the table (set 1, one binding of N sampled images, its size fixed by the
// pipeline layout) are separate descriptors: a filter change rebinds one small set and rewrites nothing. The slot is a fragment
// push constant after the vertex stage's drawable size (bytes 8 to 12), constant over a draw, so the index is dynamically
// uniform and needs no non-uniform qualifier at this stage; the extension is enabled for the unsized declaration.

layout(set = 0, binding = 0) uniform sampler scene_sampler;
layout(set = 1, binding = 0) uniform texture2D scene_images[];

layout(push_constant) uniform Batch {
    layout(offset = 8) uint slot;
} batch;

layout(location = 0) in vec2 coordinate;
layout(location = 0) out vec4 colour;

void main() {
    colour = texture(sampler2D(scene_images[batch.slot], scene_sampler), coordinate);
}
