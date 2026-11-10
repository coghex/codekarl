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
    float4 tint [[flat]];
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
    out.tint = float4(1.0f);
    return out;
}

fragment float4 ck_quad_fragment(QuadVertex in [[stage_in]], texture2d<float> image [[texture(0)]],
                                 sampler image_sampler [[sampler(0)]]) {
    return image.sample(image_sampler, in.texcoord) * in.tint;
}

/* The instanced scene (milestone 5): the same strip, the same pixel-to-clip conversion and texture-coordinate mapping, with
 * the destination (drawable pixels) and the visible part of the texture (fractions 0-1 of it) read from the instance
 * buffer by instance index. A batch is drawn with baseInstance 0 and its first instance passed explicitly, so the
 * index is `first + instance_id` whatever [[instance_id]] does with a base instance. The fragment function is shared. */

struct InstanceData {
    float4 destination; /* left, top, right, bottom, in drawable pixels */
    float4 source;      /* left, top, right, bottom, as fractions of the texture */
    uint slot;          /* the image's entry in the bindless table (the bindless pipeline only; slot 0 is the checker) */
    uchar4 tint;        /* first word of the existing uint4; record remains 48 bytes */
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
    out.tint = float4(instance.tint) / 255.0f;
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
    float4 tint [[flat]];
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
    out.tint = float4(instance.tint) / 255.0f;
    out.slot = instance.slot;
    return out;
}

fragment float4 ck_quad_bindless_fragment(BindlessVertex in [[stage_in]],
                                          const device BindlessEntry *table [[buffer(0)]],
                                          sampler checker_sampler [[sampler(0)]],
                                          sampler image_sampler [[sampler(1)]]) {
    texture2d<float> image = table[in.slot].image;
    if (in.slot == 0u) {
        return image.sample(checker_sampler, in.texcoord) * in.tint;
    }
    return image.sample(image_sampler, in.texcoord) * in.tint;
}

/* The map pass (milestone 6, slices 1, 6 and 10a): one triangle that covers the whole map target, drawn with the colour rules of
 * `docs/map-design.md` sections 9.4 and 12 (rivers are slice 8's), exactly as the reference colour function of `ck-map` computes them,
 * in integer arithmetic only: the result is the same on every GPU.
 *
 * Everything is an integer read (`read`, never `sample`). Grid A and grid B are RGBA8 images of the crop's size (A: R the terrain
 * index, G the fertility / 4 (0 to 250) or 255 for a glacier, B the hillside strength + 128; B: R and G the high and low bytes of
 * `elevation + 11000`, read for the sea's depth). The rules are one RGBA8 image of RULES_WIDTH x RULES_HEIGHT texels, written by the host
 * from the rule table the map input carries:
 *   row t < 256, terrain index t: the ten tones as given: texels 0 to 3 the shades from dark (0) to the full colour (3), 4 to 6 the
 *     shadow-side tones of shades 1 to 3, 9 to 11 the lit-side tones of shades 0 to 2; 8 = 0, kind (0 land, 1 sea, 2 lake,
 *     3 mountain), 0, 255;
 *   row 256: texels 0 sea, 1 shallow sea, 2 lake, 3 river, 4 = shallow threshold (high byte, low byte), 5 ice, 6 ice second;
 *   row 257: texel s = the map pixel size P(s) at step s, in 1/32768 cell, as three bytes (high first).
 * A unorm byte read as a float and scaled by 255 rounds back to the original integer on every GPU and on Vulkan. */

constant uint RULES_COLUMN_FLAGS = 8u;
constant uint RULES_ROW_GLOBALS = 256u;
constant uint RULES_ROW_STEPS = 257u;
constant uint KIND_SEA = 1u;
constant uint KIND_LAKE = 2u;
constant int FIXED_PER_CELL_SHIFT = 15;

/* The default look's constants (milestone 9, slice 10a; docs/map-design.md section 12). Each is pinned against `ck-map`'s `look` module by
 * a test of `ck-gpu-metal` that reads this file, so the shader and the CPU reference cannot drift apart:
 *   map_levels: the bright levels (of 256) for each stored fertility value 0 to 250, `isqrt(65536 i^5 / 250^5)` (256 at 250, so wider
 *     than a byte); entries 251 to 255 are unused;
 *   map_blue16: the 16 x 16 blue-noise tile (row = wy mod 16, column = wx mod 16), a permutation of 0 to 255;
 *   map_bayer_b: the transposed 4 x 4 Bayer matrix (row = wy mod 4, column = wx mod 4). */
constant uint map_levels[256] = {
    0u, 0u, 0u, 0u, 0u, 0u, 0u, 0u, 0u, 0u, 0u, 0u, 0u, 0u, 0u, 0u,
    0u, 0u, 0u, 0u, 0u, 0u, 0u, 0u, 0u, 0u, 0u, 0u, 1u, 1u, 1u, 1u,
    1u, 1u, 1u, 1u, 2u, 2u, 2u, 2u, 2u, 2u, 2u, 3u, 3u, 3u, 3u, 3u,
    4u, 4u, 4u, 4u, 5u, 5u, 5u, 5u, 6u, 6u, 6u, 6u, 7u, 7u, 7u, 8u,
    8u, 8u, 9u, 9u, 9u, 10u, 10u, 11u, 11u, 11u, 12u, 12u, 13u, 13u, 13u, 14u,
    14u, 15u, 15u, 16u, 16u, 17u, 17u, 18u, 18u, 19u, 19u, 20u, 21u, 21u, 22u, 22u,
    23u, 24u, 24u, 25u, 25u, 26u, 27u, 27u, 28u, 29u, 29u, 30u, 31u, 32u, 32u, 33u,
    34u, 35u, 35u, 36u, 37u, 38u, 39u, 40u, 40u, 41u, 42u, 43u, 44u, 45u, 46u, 47u,
    48u, 48u, 49u, 50u, 51u, 52u, 53u, 54u, 55u, 56u, 57u, 59u, 60u, 61u, 62u, 63u,
    64u, 65u, 66u, 67u, 69u, 70u, 71u, 72u, 73u, 75u, 76u, 77u, 78u, 80u, 81u, 82u,
    83u, 85u, 86u, 87u, 89u, 90u, 91u, 93u, 94u, 96u, 97u, 99u, 100u, 101u, 103u, 104u,
    106u, 107u, 109u, 111u, 112u, 114u, 115u, 117u, 118u, 120u, 122u, 123u, 125u, 127u, 128u, 130u,
    132u, 134u, 135u, 137u, 139u, 141u, 142u, 144u, 146u, 148u, 150u, 152u, 153u, 155u, 157u, 159u,
    161u, 163u, 165u, 167u, 169u, 171u, 173u, 175u, 177u, 179u, 181u, 183u, 185u, 188u, 190u, 192u,
    194u, 196u, 198u, 201u, 203u, 205u, 207u, 210u, 212u, 214u, 216u, 219u, 221u, 224u, 226u, 228u,
    231u, 233u, 236u, 238u, 240u, 243u, 245u, 248u, 250u, 253u, 256u, 0u, 0u, 0u, 0u, 0u
};
constant uint map_blue16[256] = {
    222u, 146u, 208u, 158u, 58u, 88u, 171u, 32u, 213u, 132u, 11u, 145u, 210u, 80u, 248u, 173u,
    67u, 5u, 102u, 251u, 27u, 197u, 140u, 65u, 184u, 236u, 54u, 95u, 24u, 182u, 36u, 99u,
    156u, 189u, 134u, 76u, 176u, 116u, 220u, 2u, 111u, 79u, 167u, 201u, 225u, 151u, 123u, 233u,
    29u, 53u, 214u, 13u, 237u, 47u, 94u, 152u, 254u, 22u, 139u, 42u, 109u, 14u, 60u, 199u,
    115u, 229u, 96u, 124u, 155u, 70u, 226u, 35u, 172u, 198u, 90u, 240u, 72u, 159u, 252u, 86u,
    168u, 143u, 39u, 203u, 23u, 190u, 131u, 207u, 64u, 122u, 8u, 181u, 216u, 127u, 186u, 3u,
    66u, 239u, 81u, 163u, 250u, 87u, 7u, 108u, 45u, 235u, 149u, 55u, 26u, 92u, 48u, 211u,
    31u, 192u, 16u, 107u, 57u, 180u, 142u, 245u, 161u, 74u, 217u, 105u, 175u, 243u, 148u, 121u,
    103u, 174u, 137u, 215u, 125u, 44u, 219u, 21u, 98u, 185u, 34u, 135u, 205u, 10u, 73u, 224u,
    50u, 247u, 63u, 9u, 234u, 165u, 78u, 200u, 129u, 1u, 230u, 82u, 114u, 40u, 196u, 157u,
    25u, 84u, 195u, 153u, 91u, 28u, 113u, 56u, 241u, 166u, 62u, 154u, 253u, 170u, 89u, 128u,
    183u, 221u, 112u, 41u, 204u, 255u, 177u, 147u, 38u, 100u, 193u, 15u, 51u, 209u, 17u, 238u,
    61u, 0u, 136u, 169u, 71u, 130u, 6u, 228u, 83u, 212u, 118u, 223u, 133u, 68u, 110u, 144u,
    162u, 104u, 246u, 20u, 218u, 49u, 106u, 188u, 18u, 141u, 30u, 178u, 93u, 242u, 187u, 37u,
    232u, 194u, 77u, 179u, 97u, 150u, 202u, 69u, 164u, 249u, 75u, 43u, 160u, 4u, 206u, 85u,
    119u, 52u, 33u, 126u, 227u, 12u, 244u, 120u, 46u, 101u, 191u, 231u, 117u, 59u, 138u, 19u
};
constant uint map_bayer_b[16] = {0u, 12u, 3u, 15u, 8u, 4u, 11u, 7u, 2u, 14u, 1u, 13u, 10u, 6u, 9u, 5u};

struct MapUniforms {
    int2 origin;   /* the crop's global origin cell (i0, j0) */
    int2 crop;     /* the crop's size in cells */
    int2 top_left; /* the camera's top-left world pixel (I_left, J_top) */
    int step;      /* the zoom step s, 0 to 75 */
    int mode;      /* 1 when a map mode shows (milestone 9, slice 8), else 0 */
};

vertex float4 ck_map_vertex(uint vertex_id [[vertex_id]]) {
    /* (-1, -1), (3, -1), (-1, 3): one triangle that covers the clip square. */
    float2 corner = float2(float((vertex_id << 1u) & 2u), float(vertex_id & 2u));
    return float4(corner * 2.0 - 1.0, 0.0, 1.0);
}

static uint4 map_bytes(float4 texel) {
    return uint4(texel * 255.0 + 0.5);
}

static uint map_u16(uint high, uint low) {
    return (high << 8u) | low;
}

/* The standard 4x4 ordered Bayer matrix, indexed by the world pixel (I mod 4, J mod 4); `& 3` is the mathematical modulo for the
 * negative pixels of a view that overlaps the crop's edge. */
static uint map_bayer(int2 pixel) {
    const uint bayer_matrix[16] = {0u, 8u, 2u, 10u, 12u, 4u, 14u, 6u, 3u, 11u, 1u, 9u, 15u, 7u, 13u, 5u};
    return bayer_matrix[uint((pixel.y & 3) * 4 + (pixel.x & 3))];
}

/* The frozen 32-bit value noise of the sub-cell detail. */
static uint map_lowbias32(uint value) {
    value ^= value >> 16u;
    value *= 0x7feb352du;
    value ^= value >> 15u;
    value *= 0x846ca68bu;
    return value ^ (value >> 16u);
}

static uint map_corner_hash(uint ix, uint iy, uint seed) {
    return map_lowbias32((ix * 0x8da6b343u) ^ (iy * 0xd8163841u) ^ (seed * 0xcb1ab31fu));
}

static uint map_smooth12(uint t) {
    uint a = (t * t) >> 12u;
    return (a * (12288u - 2u * t)) >> 12u;
}

static uint map_lerp12(uint a, uint b, uint fraction) {
    return (a * (4096u - fraction) + b * fraction) >> 12u;
}

static uint map_noise12(uint x, uint y, uint lattice, uint seed) {
    uint ix = x / lattice;
    uint iy = y / lattice;
    uint tx = map_smooth12((x % lattice) * 4096u / lattice);
    uint ty = map_smooth12((y % lattice) * 4096u / lattice);
    uint top = map_lerp12(map_corner_hash(ix, iy, seed) >> 20u, map_corner_hash(ix + 1u, iy, seed) >> 20u, tx);
    uint bottom = map_lerp12(map_corner_hash(ix, iy + 1u, seed) >> 20u, map_corner_hash(ix + 1u, iy + 1u, seed) >> 20u, tx);
    return map_lerp12(top, bottom, ty);
}

static int map_noise_offset(uint x, uint y, uint lattice, uint amplitude, uint seed) {
    return int((map_noise12(x, y, lattice, seed) * 2u * amplitude) >> 12u) - int(amplitude);
}

/* The sample point moved by the three octaves of jitter that the step has reached (first steps 14, 34 and 53). The point is inside
 * the crop, so it is not negative. */
static int2 map_jittered(int2 point, int step) {
    uint x = uint(point.x);
    uint y = uint(point.y);
    int2 moved = point;
    if (step >= 14) {
        moved += int2(map_noise_offset(x, y, 98304u, 29491u, 1u), map_noise_offset(x, y, 98304u, 29491u, 2u));
    }
    if (step >= 34) {
        moved += int2(map_noise_offset(x, y, 22938u, 9175u, 3u), map_noise_offset(x, y, 22938u, 9175u, 4u));
    }
    if (step >= 53) {
        moved += int2(map_noise_offset(x, y, 5898u, 2294u, 5u), map_noise_offset(x, y, 5898u, 2294u, 6u));
    }
    return moved;
}

static bool map_inside(constant MapUniforms &uniforms, int2 point) {
    int2 left_top = uniforms.origin << FIXED_PER_CELL_SHIFT;
    int2 right_bottom = (uniforms.origin + uniforms.crop) << FIXED_PER_CELL_SHIFT;
    return all(point >= left_top) && all(point < right_bottom);
}

/* What the cell under a world pixel's (moved) sample point is: its terrain kind and index, fertility (255: a glacier), hillside strength
 * and, for the sea, depth. */
struct MapSurface {
    bool outside;  /* the sample point (unmoved or moved) is outside the crop: sea */
    int2 moved;    /* the moved sample point */
    uint terrain;  /* the terrain index */
    uint kind;     /* KIND_* from the rules */
    uint fertility; /* grid A's green byte: fertility / 4 (0 to 250), or 255 for a glacier */
    int strength;   /* the hillside strength, -15 to +15 (positive faces the light) */
    uint depth;    /* the sea's depth: `11000 - (elevation + 11000)`, zero at or above the surface */
};

static MapSurface map_surface(constant MapUniforms &uniforms, texture2d<float, access::read> grid_a,
                              texture2d<float, access::read> grid_b, texture2d<float, access::read> rules, int2 pixel,
                              int p) {
    MapSurface surface;
    surface.outside = true;
    surface.moved = int2(0);
    surface.terrain = 0u;
    surface.kind = KIND_SEA;
    surface.fertility = 0u;
    surface.strength = 0;
    surface.depth = 0u;
    int2 unmoved = pixel * p + p / 2;
    if (!map_inside(uniforms, unmoved)) {
        return surface;
    }
    int2 moved = map_jittered(unmoved, uniforms.step);
    if (!map_inside(uniforms, moved)) {
        return surface;
    }
    surface.outside = false;
    surface.moved = moved;
    uint2 cell = uint2((moved - (uniforms.origin << FIXED_PER_CELL_SHIFT)) >> FIXED_PER_CELL_SHIFT);
    uint4 a = map_bytes(grid_a.read(cell));
    surface.terrain = a.r;
    surface.fertility = a.g;
    surface.strength = int(a.b) - 128;
    surface.kind = map_bytes(rules.read(uint2(RULES_COLUMN_FLAGS, surface.terrain))).g;
    if (surface.kind == KIND_SEA) {
        uint4 b = map_bytes(grid_b.read(cell));
        uint raw = map_u16(b.r, b.g);
        surface.depth = raw >= 11000u ? 0u : 11000u - raw;
    }
    return surface;
}

static bool map_is_water(MapSurface surface) {
    return surface.outside || surface.kind == KIND_SEA || surface.kind == KIND_LAKE;
}

/* The colour of a land pixel at world pixel `pixel` (docs/map-design.md section 12): fertility as the shade of a blue-noise tile (three
 * times the level, the tile deciding the remainder), the hillside dither of the cell's strength stepping the shade towards the
 * shadow-side or lit-side tone, a glacier's ice. `& (n - 1)` is the mathematical modulo for negative pixels. */
constant uint SHADE_STEPS = 3u;

static float4 map_land_colour(texture2d<float, access::read> rules, uint row, uint fertility, int strength, int2 pixel) {
    uint bayer = map_bayer_b[uint((pixel.y & 3) * 4 + (pixel.x & 3))];
    uint steepness = uint(abs(strength));
    bool hit = bayer < steepness;
    if (fertility == 255u) {
        return (strength < 0 && hit) ? rules.read(uint2(6u, RULES_ROW_GLOBALS)) : rules.read(uint2(5u, RULES_ROW_GLOBALS));
    }
    uint level = map_levels[min(fertility, 255u)];
    uint v = SHADE_STEPS * level;
    uint blue = map_blue16[uint((pixel.y & 15) * 16 + (pixel.x & 15))];
    uint k = min(v / 256u + ((blue < v % 256u) ? 1u : 0u), SHADE_STEPS);
    if (hit && strength < 0 && k > 0u) {
        return rules.read(uint2(3u + k, row)); /* shadow-side tone of shade k, texels 4 to 6 */
    }
    if (hit && strength > 0 && k < SHADE_STEPS) {
        return rules.read(uint2(9u + k, row)); /* lit-side tone of shade k, texels 9 to 11 */
    }
    return rules.read(uint2(k, row));
}

/* The map modes (milestone 9, slice 8; docs/economy-design.md section 3.22): the owner grid is a fourth RGBA8 image of the crop's size
 * (R and G the high and low bytes of the index of the place that owns the cell, 0xFFFF for none) and the mode's value table is a buffer
 * with one uint per place (0 to 1000, 0xFFFF for a place with nothing to show). With `uniforms.mode` set, a pixel shows the floored mean
 * of the values of the counted cells of the 5 x 5 box around the cell under its moved sample point, from red through yellow to green,
 * exactly as ck-map's `mode` module computes it; a pixel with no counted cell keeps its terrain colour. */
constant uint MODE_NONE = 0xFFFFu;
constant uint MODE_MAX = 1000u;

static float4 map_mode_colour(uint value) {
    value = min(value, MODE_MAX);
    uint3 rgb = value <= 500u ? uint3(255u, value * 255u / 500u, 0u) : uint3((MODE_MAX - value) * 255u / 500u, 255u, 0u);
    return float4(float3(rgb) / 255.0, 1.0);
}

static bool map_mode_value(constant MapUniforms &uniforms, texture2d<float, access::read> owners,
                           device const uint *values, int2 cell, thread uint &value) {
    uint sum = 0u;
    uint count = 0u;
    for (int dy = -2; dy <= 2; dy++) {
        for (int dx = -2; dx <= 2; dx++) {
            int2 at = cell + int2(dx, dy);
            if (any(at < int2(0)) || any(at >= uniforms.crop)) {
                continue;
            }
            uint4 o = map_bytes(owners.read(uint2(at)));
            uint owner = map_u16(o.r, o.g);
            if (owner == MODE_NONE) {
                continue;
            }
            uint v = values[owner];
            if (v == MODE_NONE) {
                continue;
            }
            sum += min(v, MODE_MAX);
            count++;
        }
    }
    value = count == 0u ? 0u : sum / count;
    return count != 0u;
}

fragment float4 ck_map_fragment(float4 position [[position]], constant MapUniforms &uniforms [[buffer(0)]],
                                device const uint *mode_values [[buffer(1)]],
                                texture2d<float, access::read> grid_a [[texture(0)]],
                                texture2d<float, access::read> grid_b [[texture(1)]],
                                texture2d<float, access::read> rules [[texture(2)]],
                                texture2d<float, access::read> owners [[texture(3)]]) {
    int2 pixel = uniforms.top_left + int2(position.xy);
    uint4 step_bytes = map_bytes(rules.read(uint2(uint(uniforms.step), RULES_ROW_STEPS)));
    int p = int((step_bytes.r << 16u) | (step_bytes.g << 8u) | step_bytes.b);
    uint bayer = map_bayer(pixel);
    MapSurface surface = map_surface(uniforms, grid_a, grid_b, rules, pixel, p);
    if (surface.outside) {
        return rules.read(uint2(0u, RULES_ROW_GLOBALS)); /* the sea */
    }
    if (uniforms.mode != 0) {
        uint mode_value;
        int2 cell = (surface.moved - (uniforms.origin << FIXED_PER_CELL_SHIFT)) >> FIXED_PER_CELL_SHIFT;
        if (map_mode_value(uniforms, owners, mode_values, cell, mode_value)) {
            return map_mode_colour(mode_value);
        }
    }
    if (surface.kind == KIND_LAKE) {
        return rules.read(uint2(2u, RULES_ROW_GLOBALS));
    }
    if (surface.kind == KIND_SEA) {
        uint4 limit = map_bytes(rules.read(uint2(4u, RULES_ROW_GLOBALS)));
        uint threshold = map_u16(limit.r, limit.g);
        if (surface.depth < threshold && bayer < 8u * (threshold - surface.depth) / threshold) {
            return rules.read(uint2(1u, RULES_ROW_GLOBALS)); /* the shallow sea */
        }
        return rules.read(uint2(0u, RULES_ROW_GLOBALS));
    }
    /* Land: fertility and the hillside dither (or ice), with the coast outline over both. */
    uint row = surface.terrain;
    float4 solid = rules.read(uint2(3u, row)); /* the full colour: the coast outline's */
    bool coast = false;
    int2 neighbours[4] = {int2(-1, 0), int2(1, 0), int2(0, -1), int2(0, 1)};
    for (uint n = 0u; n < 4u; n++) {
        if (map_is_water(map_surface(uniforms, grid_a, grid_b, rules, pixel + neighbours[n], p))) {
            coast = true;
        }
    }
    if (coast) {
        return solid;
    }
    return map_land_colour(rules, row, surface.fertility, surface.strength, pixel);
}

struct MapRiverUniforms {
    uint2 view;
};

/* One instanced quad per prepared river pixel. Pixel-edge positions make the overlay exactly one map pixel wide at every zoom. */
vertex float4 ck_map_river_vertex(uint vertex_id [[vertex_id]], uint instance_id [[instance_id]],
                                  constant MapRiverUniforms &uniforms [[buffer(0)]],
                                  device const uint2 *rivers [[buffer(1)]]) {
    const float2 corners[4] = {float2(0.0, 0.0), float2(0.0, 1.0), float2(1.0, 0.0), float2(1.0, 1.0)};
    float2 pixel = float2(rivers[instance_id]) + corners[vertex_id];
    float2 clip = float2(2.0 * pixel.x / float(uniforms.view.x) - 1.0,
                         1.0 - 2.0 * pixel.y / float(uniforms.view.y));
    return float4(clip, 0.0, 1.0);
}

fragment float4 ck_map_river_fragment(constant float4 &colour [[buffer(0)]]) {
    return colour;
}

struct MapLocationVertex {
    float4 position [[position]];
    uint palette_index;
};
struct MapLocation { uint x; uint y; uint palette_index; };

/* One instanced pixel quad; the location record carries its palette index through to the fragment stage. */
vertex MapLocationVertex ck_map_location_vertex(uint vertex_id [[vertex_id]], uint instance_id [[instance_id]],
                                                constant MapRiverUniforms &uniforms [[buffer(0)]],
                                                device const MapLocation *locations [[buffer(1)]]) {
    const float2 corners[4] = {float2(0.0, 0.0), float2(0.0, 1.0), float2(1.0, 0.0), float2(1.0, 1.0)};
    MapLocation location = locations[instance_id];
    float2 pixel = float2(location.x, location.y) + corners[vertex_id];
    float2 clip = float2(2.0 * pixel.x / float(uniforms.view.x) - 1.0,
                         1.0 - 2.0 * pixel.y / float(uniforms.view.y));
    return {float4(clip, 0.0, 1.0), location.palette_index};
}

fragment float4 ck_map_location_fragment(MapLocationVertex input [[stage_in]],
                                         device const uchar4 *palette [[buffer(0)]]) {
    return float4(palette[input.palette_index]) / 255.0f;
}
