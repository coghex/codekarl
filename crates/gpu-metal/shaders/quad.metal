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

/* The map pass (milestone 6, slices 1 and 6): one triangle that covers the whole map target, drawn with the full colour rules of
 * `docs/map-design.md` section 9.4 (rules 1 to 6 and 8; rivers are slice 8's), exactly as the reference colour function of `ck-map`
 * computes them, in integer arithmetic only: the result is the same on every GPU and on Vulkan.
 *
 * Everything is an integer read (`read`, never `sample`). Grid A and grid B are RGBA8 images of the crop's size (A: R the terrain
 * index, G bit 0 the usable bit; B: R and G the high and low bytes of `elevation + 11000`). The rules are one RGBA8 image of
 * RULES_WIDTH x RULES_HEIGHT texels, written by the host from the rule table the map input carries:
 *   row t < 256, terrain index t: texels 0 solid, 1 second, 2 dark, 3 dark second, 4 cap, 5 high ground (the colours as given);
 *     6 = relief low, relief high; 7 = cap low, cap high (each a 16-bit `value + 11000` as high byte, low byte); 8 = mix percent,
 *     kind (0 land, 1 sea, 2 lake, 3 mountain), 0, 255;
 *   row 256: texels 0 sea, 1 shallow sea, 2 lake, 3 river, 4 = shallow threshold (high byte, low byte);
 *   row 257: texel s = the map pixel size P(s) at step s, in 1/32768 cell, as three bytes (high first).
 * A unorm byte read as a float and scaled by 255 rounds back to the original integer on every GPU and on Vulkan. */

constant uint RULES_COLUMN_RELIEF = 6u;
constant uint RULES_COLUMN_CAP = 7u;
constant uint RULES_COLUMN_FLAGS = 8u;
constant uint RULES_ROW_GLOBALS = 256u;
constant uint RULES_ROW_STEPS = 257u;
constant uint KIND_SEA = 1u;
constant uint KIND_LAKE = 2u;
constant uint KIND_MOUNTAIN = 3u;
constant int FIXED_PER_CELL_SHIFT = 15;

struct MapUniforms {
    int2 origin;   /* the crop's global origin cell (i0, j0) */
    int2 crop;     /* the crop's size in cells */
    int2 top_left; /* the camera's top-left world pixel (I_left, J_top) */
    int step;      /* the zoom step s, 0 to 75 */
    int unused;
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

/* What the cell under a world pixel's (moved) sample point is: its terrain kind and index, usable bit and, for the sea, depth. */
struct MapSurface {
    bool outside;  /* the sample point (unmoved or moved) is outside the crop: sea */
    int2 moved;    /* the moved sample point */
    uint terrain;  /* the terrain index */
    uint kind;     /* KIND_* from the rules */
    bool usable;
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
    surface.usable = false;
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
    surface.usable = (a.g & 1u) != 0u;
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

/* The elevation in metres at the moved sample point: the four cell centres around it, interpolated in 12-bit fractions. */
static int map_elevation(constant MapUniforms &uniforms, texture2d<float, access::read> grid_b, int2 moved) {
    uint2 local = uint2(moved - (uniforms.origin << FIXED_PER_CELL_SHIFT));
    uint2 shifted = uint2(max(int2(local) - 16384, int2(0)));
    uint2 last = uint2(uniforms.crop - 1);
    uint2 low = min(shifted >> 15u, last);
    uint2 high = min(low + 1u, last);
    uint2 fraction = (shifted & 32767u) >> 3u;
    uint4 e00 = map_bytes(grid_b.read(uint2(low.x, low.y)));
    uint4 e10 = map_bytes(grid_b.read(uint2(high.x, low.y)));
    uint4 e01 = map_bytes(grid_b.read(uint2(low.x, high.y)));
    uint4 e11 = map_bytes(grid_b.read(uint2(high.x, high.y)));
    uint top = map_lerp12(map_u16(e00.r, e00.g), map_u16(e10.r, e10.g), fraction.x);
    uint bottom = map_lerp12(map_u16(e01.r, e01.g), map_u16(e11.r, e11.g), fraction.x);
    return int(map_lerp12(top, bottom, fraction.y)) - 11000;
}

/* The 16-level ramp between two elevations. */
static uint map_ramp(int value, int low, int high) {
    if (value <= low) {
        return 0u;
    }
    if (value >= high) {
        return 16u;
    }
    return uint((value - low) * 16 / (high - low));
}

static int map_signed_pair(uint4 texel, bool second) {
    return int(second ? map_u16(texel.b, texel.a) : map_u16(texel.r, texel.g)) - 11000;
}

fragment float4 ck_map_fragment(float4 position [[position]], constant MapUniforms &uniforms [[buffer(0)]],
                                texture2d<float, access::read> grid_a [[texture(0)]],
                                texture2d<float, access::read> grid_b [[texture(1)]],
                                texture2d<float, access::read> rules [[texture(2)]]) {
    int2 pixel = uniforms.top_left + int2(position.xy);
    uint4 step_bytes = map_bytes(rules.read(uint2(uint(uniforms.step), RULES_ROW_STEPS)));
    int p = int((step_bytes.r << 16u) | (step_bytes.g << 8u) | step_bytes.b);
    uint bayer = map_bayer(pixel);
    MapSurface surface = map_surface(uniforms, grid_a, grid_b, rules, pixel, p);
    if (surface.outside) {
        return rules.read(uint2(0u, RULES_ROW_GLOBALS)); /* the sea */
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
    /* Land: usable and unusable tones, the snow caps of a mountain, then the coast outline over both. */
    uint row = surface.terrain;
    float4 solid = rules.read(uint2(0u, row));
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
    int elevation = map_elevation(uniforms, grid_b, surface.moved);
    uint4 relief = map_bytes(rules.read(uint2(RULES_COLUMN_RELIEF, row)));
    uint level = map_ramp(elevation, map_signed_pair(relief, false), map_signed_pair(relief, true));
    uint mix = map_bytes(rules.read(uint2(RULES_COLUMN_FLAGS, row))).r;
    float4 colour;
    if (surface.usable) {
        colour = bayer < (level * mix) / 100u ? rules.read(uint2(1u, row)) : solid;
    } else {
        colour = bayer < level / 2u ? rules.read(uint2(3u, row)) : rules.read(uint2(2u, row));
    }
    if (surface.kind == KIND_MOUNTAIN) {
        uint4 cap = map_bytes(rules.read(uint2(RULES_COLUMN_CAP, row)));
        if (bayer < map_ramp(elevation, map_signed_pair(cap, false), map_signed_pair(cap, true))) {
            colour = surface.usable ? rules.read(uint2(4u, row)) : rules.read(uint2(5u, row));
        }
    }
    return colour;
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
