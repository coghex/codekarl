#include <metal_stdlib>
using namespace metal;

/* The textured quad (`ck-gpu-metal`'s `MetalQuad`): one triangle strip of four vertices, no vertex buffer.
 * The destination is a rectangle in drawable pixels from the top-left corner; the source is a rectangle in
 * normalised texture coordinates (also from the top-left corner, as the image's row 0 is its top). The
 * fragment returns the sampled texel unchanged: the texture holds premultiplied alpha, the pipeline blends
 * with ONE, ONE_MINUS_SRC_ALPHA, and no colour conversion happens anywhere. */

struct QuadUniforms {
    float4 destination; /* left, top, right, bottom, in drawable pixels */
    float4 source;      /* left, top, right, bottom, in texture coordinates */
    float2 viewport;    /* the drawable's width and height, in pixels */
};

struct QuadVertex {
    float4 position [[position]];
    float2 texcoord;
};

vertex QuadVertex ck_quad_vertex(uint vertex_id [[vertex_id]], constant QuadUniforms &uniforms [[buffer(0)]]) {
    /* The strip is (left, top), (right, top), (left, bottom), (right, bottom). */
    float2 corner = float2(float(vertex_id & 1u), float(vertex_id >> 1u));
    float2 pixel = mix(uniforms.destination.xy, uniforms.destination.zw, corner);
    float2 texcoord = mix(uniforms.source.xy, uniforms.source.zw, corner);
    QuadVertex out;
    out.position = float4(pixel.x / uniforms.viewport.x * 2.0 - 1.0,
                          1.0 - pixel.y / uniforms.viewport.y * 2.0, 0.0, 1.0);
    out.texcoord = texcoord;
    return out;
}

fragment float4 ck_quad_fragment(QuadVertex in [[stage_in]], texture2d<float> image [[texture(0)]],
                                 sampler image_sampler [[sampler(0)]]) {
    return image.sample(image_sampler, in.texcoord);
}

/* The instanced scene (milestone 5): the same strip, the same pixel-to-clip conversion and texture-coordinate mapping, with
 * the destination (drawable pixels) and the visible part of the texture (fractions 0-1 of it) read from the instance
 * buffer by instance index. A batch is drawn with baseInstance 0 and its first instance passed explicitly, so the
 * index is `first + instance_id` whatever [[instance_id]] does with a base instance. The fragment function is shared. */

struct InstanceData {
    float4 destination; /* left, top, right, bottom, in drawable pixels */
    float4 source;      /* left, top, right, bottom, as fractions of the texture */
    uint slot;          /* the image's entry in the bindless table (the bindless pipeline only; slot 0 is the checker) */
    uint padding0;      /* the record is 48 bytes: two float4 and a uint4 (`InstanceRecord` in batch.rs) */
    uint padding1;
    uint padding2;
};

struct InstanceUniforms {
    float2 viewport; /* the drawable's width and height, in pixels */
    uint first;      /* the batch's first instance in the instance buffer */
    uint padding;
};

vertex QuadVertex ck_quad_instance_vertex(uint vertex_id [[vertex_id]], uint instance_id [[instance_id]],
                                          constant InstanceUniforms &uniforms [[buffer(0)]],
                                          const device InstanceData *instances [[buffer(1)]]) {
    InstanceData instance = instances[uniforms.first + instance_id];
    float2 corner = float2(float(vertex_id & 1u), float(vertex_id >> 1u));
    float2 pixel = mix(instance.destination.xy, instance.destination.zw, corner);
    float2 texcoord = mix(instance.source.xy, instance.source.zw, corner);
    QuadVertex out;
    out.position = float4(pixel.x / uniforms.viewport.x * 2.0 - 1.0,
                          1.0 - pixel.y / uniforms.viewport.y * 2.0, 0.0, 1.0);
    out.texcoord = texcoord;
    return out;
}

/* The bindless scene (milestone 5, issue 10a): the same strip and mapping, with each instance's image selected by its slot in
 * the frame's table. The table is a shared-storage buffer of 8-byte MTLResourceID entries (slot 0 the checker), bound once; a
 * `BindlessEntry` is one entry, so the unsized array `const device BindlessEntry *` indexes it without a compile-time length
 * (the table's size is the configuration's `bindless_texture_slots`). The vertex function passes the slot on, flat. The
 * fragment samples the checker (slot 0) with the nearest sampler so its cells stay sharp, every other image with the frame's
 * filter, both samplers bound once; one draw is still one draw. The output is the sampled texel unchanged (premultiplied,
 * blended with ONE, ONE_MINUS_SRC_ALPHA by the pipeline, no conversion). */

struct BindlessEntry {
    texture2d<float> image [[id(0)]];
};

struct BindlessVertex {
    float4 position [[position]];
    float2 texcoord;
    uint slot [[flat]];
};

vertex BindlessVertex ck_quad_bindless_vertex(uint vertex_id [[vertex_id]], uint instance_id [[instance_id]],
                                              constant InstanceUniforms &uniforms [[buffer(0)]],
                                              const device InstanceData *instances [[buffer(1)]]) {
    InstanceData instance = instances[uniforms.first + instance_id];
    float2 corner = float2(float(vertex_id & 1u), float(vertex_id >> 1u));
    float2 pixel = mix(instance.destination.xy, instance.destination.zw, corner);
    float2 texcoord = mix(instance.source.xy, instance.source.zw, corner);
    BindlessVertex out;
    out.position = float4(pixel.x / uniforms.viewport.x * 2.0 - 1.0,
                          1.0 - pixel.y / uniforms.viewport.y * 2.0, 0.0, 1.0);
    out.texcoord = texcoord;
    out.slot = instance.slot;
    return out;
}

fragment float4 ck_quad_bindless_fragment(BindlessVertex in [[stage_in]],
                                          const device BindlessEntry *table [[buffer(0)]],
                                          sampler checker_sampler [[sampler(0)]],
                                          sampler image_sampler [[sampler(1)]]) {
    texture2d<float> image = table[in.slot].image;
    if (in.slot == 0u) {
        return image.sample(checker_sampler, in.texcoord);
    }
    return image.sample(image_sampler, in.texcoord);
}

/* The map pass (milestone 6, slice 1): one triangle that covers the whole map target, drawn with this slice's minimal colour rule.
 * Map pixel (x, y) shows the palette colour of grid A's terrain index at cell (left + x - i0, top + y - j0), one cell per map
 * pixel, and palette entry 0 outside the crop. Everything is an integer read (`read`, never `sample`): grid A is an RGBA8 image
 * of the crop's size (R the terrain index), the palette a 256 x 1 RGBA8 image. A unorm byte read as a float and scaled by 255
 * rounds back to the original integer, so the result is the same on every GPU and on Vulkan. */

struct MapUniforms {
    int2 origin;   /* the crop's global origin cell (i0, j0) */
    int2 crop;     /* the crop's size in cells */
    int2 top_left; /* the camera's top-left world pixel (I_left, J_top) */
};

vertex float4 ck_map_vertex(uint vertex_id [[vertex_id]]) {
    /* (-1, -1), (3, -1), (-1, 3): one triangle that covers the clip square. */
    float2 corner = float2(float((vertex_id << 1u) & 2u), float(vertex_id & 2u));
    return float4(corner * 2.0 - 1.0, 0.0, 1.0);
}

fragment float4 ck_map_fragment(float4 position [[position]], constant MapUniforms &uniforms [[buffer(0)]],
                                texture2d<float, access::read> grid_a [[texture(0)]],
                                texture2d<float, access::read> palette [[texture(2)]]) {
    int2 pixel = int2(position.xy);
    int2 cell = uniforms.top_left + pixel - uniforms.origin;
    uint index = 0u;
    if (all(cell >= int2(0)) && all(cell < uniforms.crop)) {
        index = uint(grid_a.read(uint2(cell)).r * 255.0 + 0.5);
    }
    return palette.read(uint2(index, 0u));
}
