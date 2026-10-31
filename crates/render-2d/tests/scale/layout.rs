//! The frozen scene and the identity-coded fixtures of the milestone 5 scale acceptance (CK-130), as pure functions.
//!
//! Nothing here touches a window, a GPU or the file system: the harness (`tests/scale.rs`) and the ordinary tests
//! (`tests/scale_pure.rs`) include this one file, so the geometry the harness draws is the geometry the pure tests judge.
//!
//! - **The full scene** (`docs/scene-design.md` §11.3): 65,536 quads in a grid of 256 x 128 cells of 7.5 x 8.4375 drawable pixels.
//!   Pass 0 shows texture `k = i` in cell `i`; pass 1 shows texture `i & !1` in the same cell offset by half a cell, right
//!   (left in the last column) and down (up in the last row), so every quad lies wholly inside the drawable and every pass-1
//!   quad overlaps its pass-0 neighbours.
//! - **The verification views**: 8 views of 64 x 64 cells (4,096 each, 30 x 16.875 drawable pixels), one pass, no overlap.
//! - **The identity code**: texture `k` is a 4 x 4 grid of equal cells encoding `k`'s 15 bits and an odd-parity bit (cell `b` holds
//!   bit `b`, row-major, bit 15 the parity), black `(0,0,0,255)` for 0 and white `(255,255,255,255)` for 1, with the top-left
//!   2 x 2 pixels half-transparent red `(255,0,0,128)`.

/// The drawable the recipe fixes, in pixels.
pub(crate) const DRAWABLE_WIDTH: u32 = 1920;
/// The drawable the recipe fixes, in pixels.
pub(crate) const DRAWABLE_HEIGHT: u32 = 1080;
/// Cell columns of the full scene.
pub(crate) const COLUMNS: u32 = 256;
/// Cell rows of the full scene.
pub(crate) const ROWS: u32 = 128;
/// A cell's width in drawable pixels (1920 / 256).
pub(crate) const CELL_WIDTH: f64 = 7.5;
/// A cell's height in drawable pixels (1080 / 128).
pub(crate) const CELL_HEIGHT: f64 = 8.4375;
/// Set A: the number of distinct 32 x 32 textures.
pub(crate) const SET_A: u32 = 32_768;
/// Quads of the full scene: two per texture.
pub(crate) const SCENE_QUADS: u32 = 2 * SET_A;
/// Side of a set A texture in pixels.
pub(crate) const SET_A_SIDE: u32 = 32;
/// Verification views and the cells of one.
pub(crate) const VIEWS: u32 = 8;
/// Cells per side of one verification view.
pub(crate) const VIEW_SIDE: u32 = 64;
/// A verification cell's width in drawable pixels (1920 / 64).
pub(crate) const VIEW_CELL_WIDTH: f64 = 30.0;
/// A verification cell's height in drawable pixels (1080 / 64).
pub(crate) const VIEW_CELL_HEIGHT: f64 = 16.875;

/// One quad of a scene, in drawable pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Placed {
    /// The set A texture it shows.
    pub(crate) texture: u32,
    /// Left edge.
    pub(crate) x: f64,
    /// Top edge.
    pub(crate) y: f64,
    /// Width.
    pub(crate) width: f64,
    /// Height.
    pub(crate) height: f64,
}

impl Placed {
    /// Whether the quad lies wholly inside the drawable.
    #[must_use]
    pub(crate) fn inside_drawable(&self) -> bool {
        self.x >= 0.0
            && self.y >= 0.0
            && self.x + self.width <= f64::from(DRAWABLE_WIDTH)
            && self.y + self.height <= f64::from(DRAWABLE_HEIGHT)
    }

    /// Whether the interiors of the two quads intersect.
    #[must_use]
    pub(crate) fn overlaps(&self, other: &Self) -> bool {
        self.x < other.x + other.width
            && other.x < self.x + self.width
            && self.y < other.y + other.height
            && other.y < self.y + self.height
    }
}

/// The cell of full-scene index `i` (0 .. 32,767): its column and row.
#[must_use]
pub(crate) fn cell_of(i: u32) -> (u32, u32) {
    (i % COLUMNS, i / COLUMNS)
}

/// Quad `index` (0 .. 65,535) of the full scene.
///
/// `offset_inside` is the recipe as frozen by r2 (F1): pass 1 moves toward the inside of the drawable at the last column and the
/// last row. `false` is r1's uniform right/down offset, kept only as the negative control of the geometry test.
#[must_use]
pub(crate) fn scene_quad(index: u32, offset_inside: bool) -> Placed {
    let (pass, i) = (index / SET_A, index % SET_A);
    let (column, row) = cell_of(i);
    let base_x = f64::from(column) * CELL_WIDTH;
    let base_y = f64::from(row) * CELL_HEIGHT;
    if pass == 0 {
        return Placed {
            texture: i,
            x: base_x,
            y: base_y,
            width: CELL_WIDTH,
            height: CELL_HEIGHT,
        };
    }
    let (left, up) = (
        offset_inside && column == COLUMNS - 1,
        offset_inside && row == ROWS - 1,
    );
    let dx = if left {
        -CELL_WIDTH / 2.0
    } else {
        CELL_WIDTH / 2.0
    };
    let dy = if up {
        -CELL_HEIGHT / 2.0
    } else {
        CELL_HEIGHT / 2.0
    };
    Placed {
        texture: i & !1,
        x: base_x + dx,
        y: base_y + dy,
        width: CELL_WIDTH,
        height: CELL_HEIGHT,
    }
}

/// Cell `index` (0 .. 4,095) of verification view `view` (0 .. 7): the texture it shows and where.
#[must_use]
pub(crate) fn view_quad(view: u32, index: u32) -> Placed {
    let (column, row) = (index % VIEW_SIDE, index / VIEW_SIDE);
    Placed {
        texture: view * VIEW_SIDE * VIEW_SIDE + index,
        x: f64::from(column) * VIEW_CELL_WIDTH,
        y: f64::from(row) * VIEW_CELL_HEIGHT,
        width: VIEW_CELL_WIDTH,
        height: VIEW_CELL_HEIGHT,
    }
}

/// The 16 code bits of texture `k`: bits 0 to 14 are `k`'s, bit 15 makes the number of ones odd.
#[must_use]
pub(crate) fn code_bits(k: u32) -> [bool; 16] {
    let mut bits = [false; 16];
    for (b, bit) in bits.iter_mut().enumerate().take(15) {
        *bit = (k >> b) & 1 == 1;
    }
    let ones = bits.iter().filter(|bit| **bit).count();
    bits[15] = ones % 2 == 0;
    bits
}

/// Decodes 16 sampled cells (bright is 1) back into the texture number, or `None` when the parity is wrong.
#[must_use]
pub(crate) fn decode_bits(bits: &[bool; 16]) -> Option<u32> {
    if bits.iter().filter(|bit| **bit).count() % 2 == 0 {
        return None;
    }
    Some(
        bits.iter()
            .take(15)
            .enumerate()
            .fold(0, |k, (b, bit)| k | (u32::from(*bit) << b)),
    )
}

/// The RGBA pixels (straight alpha, row-major) of an identity-coded `side` x `side` texture `k` (`side` a multiple of 8).
///
/// # Panics
///
/// When `side` is not a multiple of 8 (each code cell is `side / 4` pixels and the red mark is 2 pixels).
#[must_use]
pub(crate) fn identity_pixels(k: u32, side: u32) -> Vec<u8> {
    assert!(side.is_multiple_of(8), "the side must be a multiple of 8");
    let cell = side / 4;
    let bits = code_bits(k);
    let mut out = Vec::with_capacity((side * side * 4) as usize);
    for y in 0..side {
        for x in 0..side {
            if x < 2 && y < 2 {
                out.extend_from_slice(&[255, 0, 0, 128]);
                continue;
            }
            let index = ((y / cell) * 4 + x / cell) as usize;
            let value = if bits[index] { 255 } else { 0 };
            out.extend_from_slice(&[value, value, value, 255]);
        }
    }
    out
}

/// The CRC-32 of the PNG chunks.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// The Adler-32 of the zlib stream.
fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1_u32, 0_u32);
    for &byte in bytes {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) {
    out.extend_from_slice(
        &u32::try_from(data.len())
            .expect("a chunk under 4 GiB")
            .to_be_bytes(),
    );
    let mut body = kind.to_vec();
    body.extend_from_slice(data);
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc32(&body).to_be_bytes());
}

/// An RGBA8 PNG of `pixels` (`side` x `side`), deterministic: filter 0 on every row and a zlib stream of stored blocks.
///
/// # Panics
///
/// When `pixels` is not `side * side * 4` bytes.
#[must_use]
pub(crate) fn png_of(pixels: &[u8], side: u32) -> Vec<u8> {
    assert_eq!(pixels.len(), (side * side * 4) as usize, "RGBA8, square");
    let mut raw = Vec::with_capacity(pixels.len() + side as usize);
    for row in pixels.chunks((side * 4) as usize) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut zlib = vec![0x78, 0x01];
    let mut blocks = raw.chunks(65_535).peekable();
    while let Some(block) = blocks.next() {
        zlib.push(u8::from(blocks.peek().is_none()));
        let length = u16::try_from(block.len()).expect("a block of at most 65,535 bytes");
        zlib.extend_from_slice(&length.to_le_bytes());
        zlib.extend_from_slice(&(!length).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut header = Vec::new();
    header.extend_from_slice(&side.to_be_bytes());
    header.extend_from_slice(&side.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    chunk(&mut out, *b"IHDR", &header);
    chunk(&mut out, *b"IDAT", &zlib);
    chunk(&mut out, *b"IEND", &[]);
    out
}

/// The PNG of identity-coded texture `k` at `side` x `side`.
#[must_use]
pub(crate) fn identity_png(k: u32, side: u32) -> Vec<u8> {
    png_of(&identity_pixels(k, side), side)
}

/// Set B (heterogeneous, §11.3): `(count, side)` groups, in file order.
pub(crate) const SET_B: [(u32, u32); 4] = [(128, 64), (64, 256), (48, 1024), (8, 2048)];

/// Images of set B.
#[must_use]
pub(crate) fn set_b_count() -> u32 {
    SET_B.iter().map(|(count, _)| count).sum()
}

/// The side of set B image `n` (0 .. 247), in file order.
#[must_use]
pub(crate) fn set_b_side(n: u32) -> u32 {
    let mut left = n;
    for (count, side) in SET_B {
        if left < count {
            return side;
        }
        left -= count;
    }
    panic!("set B has {} images", set_b_count());
}

/// Decoded bytes of set B (RGBA8).
#[must_use]
pub(crate) fn set_b_bytes() -> u64 {
    SET_B
        .iter()
        .map(|(count, side)| u64::from(*count) * u64::from(*side) * u64::from(*side) * 4)
        .sum()
}

/// The file name of set A texture `k` in the fixtures directory.
#[must_use]
pub(crate) fn set_a_name(k: u32) -> String {
    format!("a/{k:05}.png")
}

/// The file name of set B image `n`.
#[must_use]
pub(crate) fn set_b_name(n: u32) -> String {
    format!("b/{n:03}.png")
}
