//! Building PNG files in a test: chunks, stored (uncompressed) deflate, Adam7 and lazy giant readers.
//!
//! Two test binaries include this module (`decoder` and `allocation`) and each uses a part of it.

#![expect(
    dead_code,
    reason = "two test binaries share this module and each uses a part of it"
)]

use std::collections::VecDeque;
use std::io::{self, Read};

pub(crate) const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// The colour type numbers of the IHDR chunk.
pub(crate) const GREY: u8 = 0;
pub(crate) const PALETTE: u8 = 3;
pub(crate) const RGBA: u8 = 6;

pub(crate) fn crc32(parts: &[&[u8]]) -> u32 {
    let table: Vec<u32> = (0..256_u32)
        .map(|byte| {
            (0..8).fold(byte, |value, _| {
                if value & 1 == 1 {
                    0xEDB8_8320 ^ (value >> 1)
                } else {
                    value >> 1
                }
            })
        })
        .collect();
    let mut crc = u32::MAX;
    for part in parts {
        for &byte in *part {
            crc = table[((crc ^ u32::from(byte)) & 0xFF) as usize] ^ (crc >> 8);
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut low, mut high) = (1_u32, 0_u32);
    for &byte in data {
        low = (low + u32::from(byte)) % 65521;
        high = (high + low) % 65521;
    }
    (high << 16) | low
}

pub(crate) fn chunk(kind: [u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 12);
    out.extend_from_slice(
        &u32::try_from(data.len())
            .expect("a chunk under 4 GiB")
            .to_be_bytes(),
    );
    out.extend_from_slice(&kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&[&kind, data]).to_be_bytes());
    out
}

/// A zlib stream of stored deflate blocks.
pub(crate) fn stored_zlib(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let mut blocks = data.chunks(65535).peekable();
    if blocks.peek().is_none() {
        out.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
    }
    while let Some(block) = blocks.next() {
        let length = u16::try_from(block.len()).expect("a stored block");
        out.push(u8::from(blocks.peek().is_none()));
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&(!length).to_le_bytes());
        out.extend_from_slice(block);
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

/// The IHDR of an image: width, height, bit depth, colour type, interlace method.
pub(crate) fn ihdr(width: u32, height: u32, depth: u8, color: u8, interlace: u8) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(&width.to_be_bytes());
    data.extend_from_slice(&height.to_be_bytes());
    data.extend_from_slice(&[depth, color, 0, 0, interlace]);
    chunk(*b"IHDR", &data)
}

/// The signature and a header: all a decoder needs to refuse an image by its size.
pub(crate) fn header_only(width: u32, height: u32, depth: u8, color: u8) -> Vec<u8> {
    [SIGNATURE.to_vec(), ihdr(width, height, depth, color, 0)].concat()
}

/// A whole file: the signature, the header, `middle` chunks, one IDAT of `scanlines`, and IEND.
pub(crate) fn file(header: &[u8], middle: &[Vec<u8>], scanlines: &[u8]) -> Vec<u8> {
    let mut out = SIGNATURE.to_vec();
    out.extend_from_slice(header);
    for chunk in middle {
        out.extend_from_slice(chunk);
    }
    out.extend_from_slice(&chunk(*b"IDAT", &stored_zlib(scanlines)));
    out.extend_from_slice(&chunk(*b"IEND", &[]));
    out
}

const ADAM7: [(usize, usize, usize, usize); 7] = [
    (0, 0, 8, 8),
    (4, 0, 8, 8),
    (0, 4, 4, 8),
    (2, 0, 4, 4),
    (0, 2, 2, 4),
    (1, 0, 2, 2),
    (0, 1, 1, 2),
];

/// Scanlines (filter 0) of an image whose pixels are `bytes` bytes each, written by `pixel(x, y, out)`, in file
/// order: rows top to bottom, or the seven Adam7 passes.
pub(crate) fn scanlines(
    width: usize,
    height: usize,
    bytes: usize,
    interlaced: bool,
    pixel: impl Fn(usize, usize, &mut [u8]),
) -> Vec<u8> {
    let passes: &[(usize, usize, usize, usize)] = if interlaced { &ADAM7 } else { &[(0, 0, 1, 1)] };
    let mut out = Vec::new();
    for &(x0, y0, dx, dy) in passes {
        for y in (y0..height).step_by(dy) {
            if x0 >= width {
                continue;
            }
            out.push(0);
            for x in (x0..width).step_by(dx) {
                let at = out.len();
                out.resize(at + bytes, 0);
                pixel(x, y, &mut out[at..]);
            }
        }
    }
    out
}

/// A piece of a lazily-read file.
pub(crate) enum Part {
    Bytes(Vec<u8>),
    /// This many zero bytes, never held in memory.
    Zeros(u64),
}

/// Reads its parts in order, so a file of any length can be read without being built.
pub(crate) struct Parts {
    parts: VecDeque<Part>,
    offset: usize,
}

impl Parts {
    pub(crate) fn new(parts: Vec<Part>) -> Self {
        Self {
            parts: parts.into(),
            offset: 0,
        }
    }

    /// A chunk header and `length` zero bytes of data and a (wrong, unchecked) checksum, as parts.
    pub(crate) fn giant_chunk(kind: [u8; 4], length: u64) -> Vec<Part> {
        let mut header = u32::try_from(length)
            .expect("a chunk under 4 GiB")
            .to_be_bytes()
            .to_vec();
        header.extend_from_slice(&kind);
        vec![
            Part::Bytes(header),
            Part::Zeros(length),
            Part::Bytes(vec![0; 4]),
        ]
    }
}

impl Read for Parts {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            match self.parts.front_mut() {
                None => return Ok(0),
                Some(Part::Bytes(bytes)) => {
                    let count = (bytes.len() - self.offset).min(buffer.len());
                    buffer[..count].copy_from_slice(&bytes[self.offset..self.offset + count]);
                    self.offset += count;
                    if self.offset == bytes.len() {
                        self.parts.pop_front();
                        self.offset = 0;
                    }
                    if count > 0 {
                        return Ok(count);
                    }
                }
                Some(Part::Zeros(remaining)) => {
                    let count = usize::try_from(*remaining)
                        .map_or(buffer.len(), |left| left.min(buffer.len()));
                    buffer[..count].fill(0);
                    *remaining -= count as u64;
                    if *remaining == 0 {
                        self.parts.pop_front();
                    }
                    if count > 0 {
                        return Ok(count);
                    }
                }
            }
        }
    }
}

/// A reader that counts the bytes read from it.
pub(crate) struct Counting<R> {
    pub(crate) inner: R,
    pub(crate) read: u64,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.inner.read(buffer)?;
        self.read += count as u64;
        Ok(count)
    }
}
