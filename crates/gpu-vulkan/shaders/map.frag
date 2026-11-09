#version 450
#extension GL_EXT_samplerless_texture_functions : require
// The map pass's fragment stage (milestone 6, slices 1 and 6): the full colour rules of `docs/map-design.md` section 9.4 (rules 1 to
// 6 and 8; rivers are slice 8's), exactly as the reference colour function of `ck-map` computes them, in integer arithmetic only: the
// result is the same on every GPU and on Metal.
//
// Everything is an integer read (`texelFetch`, never a filtered sample). Grid A and grid B are RGBA8 images of the crop's size
// (A: R the terrain index, G bit 0 the usable bit; B: R and G the high and low bytes of `elevation + 11000`). The rules are one RGBA8
// image of 80 x 258 texels, written by the host from the rule table the map input carries:
//   row t < 256, terrain index t: texels 0 solid, 1 second, 2 dark, 3 dark second, 4 cap, 5 high ground (the colours as given);
//     6 = relief low, relief high; 7 = cap low, cap high (each a 16-bit `value + 11000` as high byte, low byte); 8 = mix percent,
//     kind (0 land, 1 sea, 2 lake, 3 mountain), 0, 255;
//   row 256: texels 0 sea, 1 shallow sea, 2 lake, 3 river, 4 = shallow threshold (high byte, low byte);
//   row 257: texel s = the map pixel size P(s) at step s, in 1/32768 cell, as three bytes (high first).
// The map modes (milestone 9, slice 8; `docs/economy-design.md` section 3.22): the owner grid is a fourth RGBA8 image of the crop's size
// (binding 4: R and G the high and low bytes of the index of the place that owns the cell, 0xFFFF for none), and the mode's value table
// is a storage buffer (set 1) with one uint per place (0 to 1000, 0xFFFF for a place with nothing to show). With `camera.mode` set, a
// pixel shows the floored mean of the values of the counted cells of the 5 x 5 box around the cell under its moved sample point, from
// red through yellow to green, exactly as `ck-map`'s `mode` module computes it; a pixel with no counted cell keeps its terrain colour.
//
// A unorm byte read as a float and scaled by 255 rounds back to the original integer on every GPU and on Metal. `gl_FragCoord` has
// its origin at the target's top-left corner and a half at the pixel centre, so its integer part is the pixel's column and row.

layout(push_constant) uniform Camera {
    ivec2 origin;   // the crop's global origin cell (i0, j0)
    ivec2 crop;     // the crop's size in cells
    ivec2 top_left; // the camera's top-left world pixel (I_left, J_top)
    int zoom;       // the zoom step s, 0 to 75
    int mode;       // 1 when a map mode shows, else 0
} camera;

layout(set = 0, binding = 0) uniform texture2D grid_a;
layout(set = 0, binding = 1) uniform texture2D grid_b;
layout(set = 0, binding = 2) uniform texture2D rules;
layout(set = 0, binding = 4) uniform texture2D owners;
layout(set = 1, binding = 0) readonly buffer ModeValues {
    uint values[];
} mode_values;

layout(location = 0) out vec4 colour;

const uint RULES_COLUMN_RELIEF = 6u;
const uint RULES_COLUMN_CAP = 7u;
const uint RULES_COLUMN_FLAGS = 8u;
const uint RULES_ROW_GLOBALS = 256u;
const uint RULES_ROW_STEPS = 257u;
const uint KIND_SEA = 1u;
const uint KIND_LAKE = 2u;
const uint KIND_MOUNTAIN = 3u;
const int FIXED_PER_CELL_SHIFT = 15;
const uint MODE_NONE = 0xFFFFu;
const uint MODE_MAX = 1000u;

uvec4 map_bytes(vec4 texel) {
    return uvec4(texel * 255.0 + 0.5);
}

uint map_u16(uint high, uint low) {
    return (high << 8) | low;
}

vec4 rule_texel(uint column, uint row) {
    return texelFetch(rules, ivec2(int(column), int(row)), 0);
}

// The standard 4x4 ordered Bayer matrix, indexed by the world pixel (I mod 4, J mod 4); `& 3` is the mathematical modulo for the
// negative pixels of a view that overlaps the crop's edge.
uint map_bayer(ivec2 pixel) {
    const uint bayer_matrix[16] = uint[16](0u, 8u, 2u, 10u, 12u, 4u, 14u, 6u, 3u, 11u, 1u, 9u, 15u, 7u, 13u, 5u);
    return bayer_matrix[(pixel.y & 3) * 4 + (pixel.x & 3)];
}

// The frozen 32-bit value noise of the sub-cell detail.
uint map_lowbias32(uint value) {
    value ^= value >> 16;
    value *= 0x7feb352du;
    value ^= value >> 15;
    value *= 0x846ca68bu;
    return value ^ (value >> 16);
}

uint map_corner_hash(uint ix, uint iy, uint seed) {
    return map_lowbias32((ix * 0x8da6b343u) ^ (iy * 0xd8163841u) ^ (seed * 0xcb1ab31fu));
}

uint map_smooth12(uint t) {
    uint a = (t * t) >> 12;
    return (a * (12288u - 2u * t)) >> 12;
}

uint map_lerp12(uint a, uint b, uint fraction) {
    return (a * (4096u - fraction) + b * fraction) >> 12;
}

uint map_noise12(uint x, uint y, uint lattice, uint seed) {
    uint ix = x / lattice;
    uint iy = y / lattice;
    uint tx = map_smooth12((x % lattice) * 4096u / lattice);
    uint ty = map_smooth12((y % lattice) * 4096u / lattice);
    uint top = map_lerp12(map_corner_hash(ix, iy, seed) >> 20, map_corner_hash(ix + 1u, iy, seed) >> 20, tx);
    uint bottom = map_lerp12(map_corner_hash(ix, iy + 1u, seed) >> 20, map_corner_hash(ix + 1u, iy + 1u, seed) >> 20, tx);
    return map_lerp12(top, bottom, ty);
}

int map_noise_offset(uint x, uint y, uint lattice, uint amplitude, uint seed) {
    return int((map_noise12(x, y, lattice, seed) * 2u * amplitude) >> 12) - int(amplitude);
}

// The sample point moved by the three octaves of jitter that the step has reached (first steps 14, 34 and 53). The point is inside
// the crop, so it is not negative.
ivec2 map_jittered(ivec2 point) {
    uint x = uint(point.x);
    uint y = uint(point.y);
    ivec2 moved = point;
    if (camera.zoom >= 14) {
        moved += ivec2(map_noise_offset(x, y, 98304u, 29491u, 1u), map_noise_offset(x, y, 98304u, 29491u, 2u));
    }
    if (camera.zoom >= 34) {
        moved += ivec2(map_noise_offset(x, y, 22938u, 9175u, 3u), map_noise_offset(x, y, 22938u, 9175u, 4u));
    }
    if (camera.zoom >= 53) {
        moved += ivec2(map_noise_offset(x, y, 5898u, 2294u, 5u), map_noise_offset(x, y, 5898u, 2294u, 6u));
    }
    return moved;
}

bool map_inside(ivec2 point) {
    ivec2 left_top = camera.origin << FIXED_PER_CELL_SHIFT;
    ivec2 right_bottom = (camera.origin + camera.crop) << FIXED_PER_CELL_SHIFT;
    return all(greaterThanEqual(point, left_top)) && all(lessThan(point, right_bottom));
}

// What the cell under a world pixel's (moved) sample point is: its terrain kind and index, usable bit and, for the sea, depth.
struct MapSurface {
    bool outside;  // the sample point (unmoved or moved) is outside the crop: sea
    ivec2 moved;   // the moved sample point
    uint terrain;  // the terrain index
    uint kind;     // KIND_* from the rules
    bool usable;
    uint depth;    // the sea's depth: `11000 - (elevation + 11000)`, zero at or above the surface
};

MapSurface map_surface(ivec2 pixel, int p) {
    MapSurface surface;
    surface.outside = true;
    surface.moved = ivec2(0);
    surface.terrain = 0u;
    surface.kind = KIND_SEA;
    surface.usable = false;
    surface.depth = 0u;
    ivec2 unmoved = pixel * p + p / 2;
    if (!map_inside(unmoved)) {
        return surface;
    }
    ivec2 moved = map_jittered(unmoved);
    if (!map_inside(moved)) {
        return surface;
    }
    surface.outside = false;
    surface.moved = moved;
    ivec2 cell = (moved - (camera.origin << FIXED_PER_CELL_SHIFT)) >> FIXED_PER_CELL_SHIFT;
    uvec4 a = map_bytes(texelFetch(grid_a, cell, 0));
    surface.terrain = a.r;
    surface.usable = (a.g & 1u) != 0u;
    surface.kind = map_bytes(rule_texel(RULES_COLUMN_FLAGS, surface.terrain)).g;
    if (surface.kind == KIND_SEA) {
        uvec4 b = map_bytes(texelFetch(grid_b, cell, 0));
        uint raw = map_u16(b.r, b.g);
        surface.depth = raw >= 11000u ? 0u : 11000u - raw;
    }
    return surface;
}

bool map_is_water(MapSurface surface) {
    return surface.outside || surface.kind == KIND_SEA || surface.kind == KIND_LAKE;
}

uint map_raw_elevation(uint x, uint y) {
    uvec4 e = map_bytes(texelFetch(grid_b, ivec2(int(x), int(y)), 0));
    return map_u16(e.r, e.g);
}

// The elevation in metres at the moved sample point: the four cell centres around it, interpolated in 12-bit fractions.
int map_elevation(ivec2 moved) {
    ivec2 local = moved - (camera.origin << FIXED_PER_CELL_SHIFT);
    uvec2 shifted = uvec2(max(local - 16384, ivec2(0)));
    uvec2 last = uvec2(camera.crop - 1);
    uvec2 low = min(shifted >> 15, last);
    uvec2 high = min(low + 1u, last);
    uvec2 fraction = (shifted & 32767u) >> 3;
    uint top = map_lerp12(map_raw_elevation(low.x, low.y), map_raw_elevation(high.x, low.y), fraction.x);
    uint bottom = map_lerp12(map_raw_elevation(low.x, high.y), map_raw_elevation(high.x, high.y), fraction.x);
    return int(map_lerp12(top, bottom, fraction.y)) - 11000;
}

// The 16-level ramp between two elevations.
uint map_ramp(int value, int low, int high) {
    if (value <= low) {
        return 0u;
    }
    if (value >= high) {
        return 16u;
    }
    return uint((value - low) * 16 / (high - low));
}

int map_signed_pair(uvec4 texel, bool second) {
    return int(second ? map_u16(texel.b, texel.a) : map_u16(texel.r, texel.g)) - 11000;
}

// The colour of a map-mode value: red (255, 0, 0) at 0, yellow (255, 255, 0) at 500, green (0, 255, 0) at 1000, linear and floored.
vec4 map_mode_colour(uint value) {
    value = min(value, MODE_MAX);
    uvec3 rgb = value <= 500u ? uvec3(255u, value * 255u / 500u, 0u) : uvec3((MODE_MAX - value) * 255u / 500u, 255u, 0u);
    return vec4(vec3(rgb) / 255.0, 1.0);
}

// Whether the cell has a counted cell in its 5 x 5 box, and the floored mean of their values. A cell is counted when a place owns it
// and that place has a value; the box is clipped to the crop.
bool map_mode_value(ivec2 cell, out uint value) {
    uint sum = 0u;
    uint count = 0u;
    for (int dy = -2; dy <= 2; dy++) {
        for (int dx = -2; dx <= 2; dx++) {
            ivec2 at = cell + ivec2(dx, dy);
            if (any(lessThan(at, ivec2(0))) || any(greaterThanEqual(at, camera.crop))) {
                continue;
            }
            uvec4 o = map_bytes(texelFetch(owners, at, 0));
            uint owner = map_u16(o.r, o.g);
            if (owner == MODE_NONE) {
                continue;
            }
            uint v = mode_values.values[owner];
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

void main() {
    ivec2 pixel = camera.top_left + ivec2(gl_FragCoord.xy);
    uvec4 step_bytes = map_bytes(rule_texel(uint(camera.zoom), RULES_ROW_STEPS));
    int p = int((step_bytes.r << 16) | (step_bytes.g << 8) | step_bytes.b);
    uint bayer = map_bayer(pixel);
    MapSurface surface = map_surface(pixel, p);
    if (surface.outside) {
        colour = rule_texel(0u, RULES_ROW_GLOBALS); // the sea
        return;
    }
    if (camera.mode != 0) {
        uint mode_value;
        ivec2 cell = (surface.moved - (camera.origin << FIXED_PER_CELL_SHIFT)) >> FIXED_PER_CELL_SHIFT;
        if (map_mode_value(cell, mode_value)) {
            colour = map_mode_colour(mode_value);
            return;
        }
    }
    if (surface.kind == KIND_LAKE) {
        colour = rule_texel(2u, RULES_ROW_GLOBALS);
        return;
    }
    if (surface.kind == KIND_SEA) {
        uvec4 limit = map_bytes(rule_texel(4u, RULES_ROW_GLOBALS));
        uint threshold = map_u16(limit.r, limit.g);
        if (surface.depth < threshold && bayer < 8u * (threshold - surface.depth) / threshold) {
            colour = rule_texel(1u, RULES_ROW_GLOBALS); // the shallow sea
        } else {
            colour = rule_texel(0u, RULES_ROW_GLOBALS);
        }
        return;
    }
    // Land: usable and unusable tones, the snow caps of a mountain, then the coast outline over both.
    uint row = surface.terrain;
    vec4 solid = rule_texel(0u, row);
    ivec2 neighbours[4] = ivec2[4](ivec2(-1, 0), ivec2(1, 0), ivec2(0, -1), ivec2(0, 1));
    for (int n = 0; n < 4; n++) {
        if (map_is_water(map_surface(pixel + neighbours[n], p))) {
            colour = solid;
            return;
        }
    }
    int elevation = map_elevation(surface.moved);
    uvec4 relief = map_bytes(rule_texel(RULES_COLUMN_RELIEF, row));
    uint level = map_ramp(elevation, map_signed_pair(relief, false), map_signed_pair(relief, true));
    uint mix_percent = map_bytes(rule_texel(RULES_COLUMN_FLAGS, row)).r;
    if (surface.usable) {
        colour = bayer < (level * mix_percent) / 100u ? rule_texel(1u, row) : solid;
    } else {
        colour = bayer < level / 2u ? rule_texel(3u, row) : rule_texel(2u, row);
    }
    if (surface.kind == KIND_MOUNTAIN) {
        uvec4 cap = map_bytes(rule_texel(RULES_COLUMN_CAP, row));
        if (bayer < map_ramp(elevation, map_signed_pair(cap, false), map_signed_pair(cap, true))) {
            colour = surface.usable ? rule_texel(4u, row) : rule_texel(5u, row);
        }
    }
}
