#version 450
#extension GL_EXT_nonuniform_qualifier : require
// The scene's fragment stage through the bindless table (milestone 5.5, issues 5 and 6): the sampled texel as it is, from the table's
// entry for the instance's slot. The image is premultiplied RGBA8 and the pipeline blends with ONE, ONE_MINUS_SRC_ALPHA, so
// nothing is multiplied here. Set 0 holds two samplers (binding 0 the checker's nearest, binding 1 the frame's filter) and set 1 the
// table (one binding of N sampled images, its size fixed by the pipeline layout).
//
// The slot comes from the instance, so it varies between the fragments of one draw: the index is NON-UNIFORM (design §13.1). The
// qualifier must reach the resource that is actually sampled, not a copy of the index: GL_EXT_nonuniform_qualifier does not propagate it
// through an assignment, and a constructor's result needs it again. So `nonuniformEXT` is applied at the descriptor index and again at
// the constructed combined sampler, on both branches, with no intermediate variable (the compiled module carries `NonUniform` on the
// access chain, the loaded image and the sampled image; `runtime-m55/ck162-partA-spirv-disassembly.txt` retains the disassembly).
// Slot 0 is the checker, which samples nearest (it must stay sharp under a linear frame); every other slot samples the frame's filter.
// The images have one mip level, so `textureLod(.., 0.0)` is exactly the texel `texture` returns, and it needs no implicit derivatives
// in the divergent control flow.

layout(set = 0, binding = 0) uniform sampler checker_sampler;
layout(set = 0, binding = 1) uniform sampler frame_sampler;
layout(set = 1, binding = 0) uniform texture2D scene_images[];

layout(location = 0) in vec2 coordinate;
layout(location = 1) flat in uint table_slot;
layout(location = 0) out vec4 colour;

void main() {
    if (table_slot == 0u) {
        colour = textureLod(
            nonuniformEXT(sampler2D(scene_images[nonuniformEXT(table_slot)], checker_sampler)),
            coordinate, 0.0);
    } else {
        colour = textureLod(
            nonuniformEXT(sampler2D(scene_images[nonuniformEXT(table_slot)], frame_sampler)),
            coordinate, 0.0);
    }
}
