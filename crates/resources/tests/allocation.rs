//! The decoder's allocation envelope, measured.
//!
//! This binary has no test harness, so it is one thread running one scenario at a time. A counting global
//! allocator keeps the **total** number of bytes live in the process; a measurement is the most that total
//! reaches during one decode, minus what was live before it (the input, built beforehand, is in the baseline).
//!
//! **The envelope** of a decode is the final image, plus the caller's intermediate (one raw row plus the 64 KiB
//! read buffer), plus what the `png` crate may allocate for itself (its 16 MiB limit), plus 1 MiB of slack for
//! small bookkeeping. The caller's own allocation is not measured apart from the library's: the total covers
//! both, and each scenario states what it shows.

use std::alloc::{GlobalAlloc, Layout, System};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use ck_resources::budget::{
    DECODER_INTERNAL_BYTES, MAX_DECODED_BYTES, MAX_ENCODED_BYTES, READ_BUFFER_BYTES,
};
use ck_resources::{
    Assets, DecodeError, DecodeLimits, Image, LegacyAdmitError, LegacyBudget, Mode, ReserveError,
    ResourceLimits, ResourceService, ServiceWakes, TooLarge,
};

#[path = "support/png.rs"]
mod png;

use png::{Counting, GREY, Part, Parts, RGBA};

const MIB: usize = 1024 * 1024;

/// Bytes live now, and the most they have been since the last measurement began.
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

struct CountingAllocator;

fn grow(bytes: usize) {
    let live = LIVE.fetch_add(bytes, Ordering::SeqCst) + bytes;
    PEAK.fetch_max(live, Ordering::SeqCst);
}

// SAFETY: every call goes straight to the system allocator with the arguments it was given, so that allocator's
// guarantees are the ones this one gives; the bookkeeping is in atomics and allocates nothing.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::alloc`'s contract for `layout`, unchanged here.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            grow(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::alloc_zeroed`'s contract for `layout`, unchanged here.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            grow(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: the caller upholds `GlobalAlloc::dealloc`'s contract: `pointer` came from this allocator with
        // `layout`, and both are passed on unchanged.
        unsafe { System.dealloc(pointer, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::SeqCst);
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::realloc`'s contract; everything is passed on unchanged.
        let moved = unsafe { System.realloc(pointer, layout, new_size) };
        if !moved.is_null() {
            LIVE.fetch_sub(layout.size(), Ordering::SeqCst);
            grow(new_size);
        }
        moved
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Runs `body` and returns its result and the most bytes live during it above those live before it.
fn measure<T>(body: impl FnOnce() -> T) -> (T, usize) {
    let baseline = LIVE.load(Ordering::SeqCst);
    PEAK.store(baseline, Ordering::SeqCst);
    let result = body();
    (result, PEAK.load(Ordering::SeqCst) - baseline)
}

/// What one decode may hold at its peak, above what was live before it.
#[derive(Debug, Clone, Copy)]
struct Envelope {
    /// The final RGBA8 image.
    image: usize,
    /// One raw row of the file's own format.
    row: usize,
    /// The read buffer.
    buffer: usize,
    /// What the `png` crate may allocate for itself.
    library: usize,
    /// Small bookkeeping.
    slack: usize,
}

impl Envelope {
    /// The envelope of an image `width` by `height` whose raw rows take `row_bytes`.
    fn of(width: usize, height: usize, row_bytes: usize) -> Self {
        Self {
            image: width * height * 4,
            row: row_bytes,
            buffer: READ_BUFFER_BYTES,
            library: DECODER_INTERNAL_BYTES,
            slack: MIB,
        }
    }

    fn total(self) -> usize {
        self.image + self.row + self.buffer + self.library + self.slack
    }

    /// Everything but the image.
    fn intermediate(self) -> usize {
        self.total() - self.image
    }

    fn contains(self, peak: usize) -> bool {
        peak <= self.total()
    }
}

fn say(scenario: &str, peak: usize, bound: &str) {
    println!("allocation: {scenario}: peak {peak} bytes ({bound})");
}

/// Three IHDR-only files that the header alone refuses: wide, over the decoded cap by a row, and both at once.
fn refused_by_the_header_alone() {
    for (width, height, depth, color) in [
        (8193, 1, 8, GREY),
        (4097, 4096, 8, GREY),
        (8192, 2049, 16, RGBA),
    ] {
        let bytes = png::header_only(width, height, depth, color);
        let mut source = Counting {
            inner: bytes.as_slice(),
            read: 0,
        };
        let (error, peak) = measure(|| Image::decode(&mut source).expect_err("a refusal"));
        assert!(
            matches!(error, DecodeError::TooLarge(_)),
            "{width} x {height}: {error}"
        );
        assert_eq!(
            source.read,
            bytes.len() as u64,
            "{width} x {height}: only the header was read"
        );
        // Far below any image-sized allocation: the read buffer and a little bookkeeping.
        assert!(peak < 256 * 1024, "{width} x {height}: peak {peak}");
        say(
            &format!("header {width} x {height} refused"),
            peak,
            "less than 256 KiB",
        );
    }
}

/// A 1 x 1 grey image whose 70 MiB of text is over the encoded cap: refused, having read the cap and one byte.
fn over_the_encoded_cap() {
    let mut parts = vec![Part::Bytes(png::header_only(1, 1, 8, GREY))];
    parts.extend(Parts::giant_chunk(*b"tEXt", 70 * MIB as u64));
    parts.push(Part::Bytes(
        [
            png::chunk(*b"IDAT", &png::stored_zlib(&[0, 7])),
            png::chunk(*b"IEND", &[]),
        ]
        .concat(),
    ));
    let mut source = Counting {
        inner: Parts::new(parts),
        read: 0,
    };
    let (error, peak) = measure(|| Image::decode(&mut source).expect_err("a refusal"));
    assert_eq!(
        error,
        DecodeError::TooLarge(TooLarge::File {
            limit: MAX_ENCODED_BYTES
        })
    );
    assert_eq!(source.read, MAX_ENCODED_BYTES + 1);
    assert!(peak < 256 * 1024, "peak {peak}");
    say(
        "70 MiB of text over the encoded cap, refused",
        peak,
        "less than 256 KiB",
    );
}

/// A 1 x 1 grey image with 42 MiB of text, ICC, EXIF and unknown chunks around its data: under the encoded cap,
/// so it decodes, and the metadata is skipped (it is more than the envelope's whole allowance for the library).
fn large_metadata_under_the_cap() {
    let mut parts = vec![Part::Bytes(png::header_only(1, 1, 8, GREY))];
    for kind in [
        *b"tEXt", *b"zTXt", *b"iTXt", *b"iCCP", *b"eXIf", *b"prVt", *b"gAMA",
    ] {
        parts.extend(Parts::giant_chunk(kind, 6 * MIB as u64));
    }
    parts.push(Part::Bytes(
        [
            png::chunk(*b"IDAT", &png::stored_zlib(&[0, 7])),
            png::chunk(*b"IEND", &[]),
        ]
        .concat(),
    ));
    let (image, peak) = measure(|| Image::decode(Parts::new(parts)).expect("a decode"));
    assert_eq!(
        (image.width(), image.height(), image.pixels()),
        (1, 1, &[7, 7, 7, 255][..])
    );
    let envelope = Envelope::of(1, 1, 1);
    assert!(envelope.contains(peak), "peak {peak} outside {envelope:?}");
    // A stored chunk would be 6 MiB: the real peak is a tenth of one.
    assert!(peak < MIB, "peak {peak}");
    say(
        "42 MiB of metadata around a 1 x 1 image",
        peak,
        "inside the envelope, and under 1 MiB",
    );
}

/// 4096 x 4096 one-bit grey: a 2 MiB file that decodes to exactly the 64 MiB decoded cap.
fn the_decoded_cap_file() -> Vec<u8> {
    let mut rows = Vec::with_capacity(4096 * 513);
    for y in 0..4096_usize {
        rows.push(0);
        rows.extend((0..512).map(|byte| if (y + byte) % 2 == 0 { 0xF0 } else { 0x0F }));
    }
    let bytes = png::file(&png::ihdr(4096, 4096, 1, GREY, 0), &[], &rows);
    drop(rows);
    bytes
}

/// A reservation refused for a large image leaves the peak far below the image: decoding stops at the reservation, with the
/// read buffer, a row and the library's allowance as the only allocations; an accepted one stays within the existing
/// envelope and returns what it was given.
fn a_refused_reservation_allocates_nothing_image_sized() {
    let bytes = the_decoded_cap_file();
    let envelope = Envelope::of(4096, 4096, 512);
    let (refused, peak) = measure(|| {
        Image::decode_reserving(bytes.as_slice(), &DecodeLimits::SHIPPED, |size| {
            Err::<(), u64>(size)
        })
        .expect_err("a refusal")
    });
    assert_eq!(
        refused,
        ReserveError::Refused(MAX_DECODED_BYTES as u64),
        "refused with the size it was asked for"
    );
    // The stated bound: everything but the image.
    assert!(
        peak <= envelope.intermediate(),
        "peak {peak} above the read buffer, a row, the library allowance and the slack ({})",
        envelope.intermediate()
    );
    assert!(peak < MAX_DECODED_BYTES / 4, "far below the image: {peak}");
    say(
        "4096 x 4096 with the reservation refused",
        peak,
        &format!(
            "at most {} and far below the 64 MiB image",
            envelope.intermediate()
        ),
    );
    let ((image, granted), accepted) = measure(|| {
        Image::decode_reserving(bytes.as_slice(), &DecodeLimits::SHIPPED, |size| {
            Ok::<u64, ()>(size)
        })
        .expect("a decode")
    });
    assert_eq!(granted, MAX_DECODED_BYTES as u64);
    assert_eq!(image.pixels().len(), MAX_DECODED_BYTES);
    assert!(
        envelope.contains(accepted),
        "peak {accepted} outside {envelope:?}"
    );
    assert!(
        accepted >= MAX_DECODED_BYTES,
        "the image itself: {accepted}"
    );
    say(
        "4096 x 4096 with the reservation granted",
        accepted,
        &format!("envelope {}", envelope.total()),
    );
}

/// The service's decoded-image admission uses the decoder reservation callback, so a CPU-budget refusal stays below the
/// image-sized allocation. Negative control: the legacy decoder without reservation allocates the full image.
fn create_decoded_reserves_before_image_allocation() {
    const PIXEL_CAP: u64 = MAX_DECODED_BYTES as u64;
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let root: PathBuf = std::env::temp_dir().join(format!(
        "ck-resources-allocation-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    fs::create_dir(&root).expect("a unique scratch root");
    let bytes = the_decoded_cap_file();
    fs::write(root.join("large.png"), &bytes).expect("the encoded image");

    let mut limits = ResourceLimits::DEFAULT;
    limits.cpu_pixel_bytes = PIXEL_CAP;
    let service = ResourceService::with_assets(
        limits,
        Mode::Backend,
        ServiceWakes::default(),
        Some(Assets::new(&root)),
    )
    .expect("a bounded backend service");
    let held = service.reserve_pixels(1, 1).expect("hold four CPU bytes");
    let envelope = Envelope::of(4096, 4096, 512);
    let (error, peak) = measure(|| {
        service
            .create_decoded("large.png")
            .expect_err("one held pixel leaves too little room for the 64 MiB image")
    });
    assert!(
        matches!(
            error,
            LegacyAdmitError::Budget {
                budget: LegacyBudget::CpuPixelBytes,
                requested,
                available,
            }
                if requested == PIXEL_CAP && available == PIXEL_CAP - 4
        ),
        "{error:?}"
    );
    assert!(
        peak <= envelope.intermediate(),
        "create_decoded peak {peak} exceeded intermediate envelope {}",
        envelope.intermediate()
    );
    assert!(
        peak < MAX_DECODED_BYTES / 4,
        "image-sized allocation: {peak}"
    );
    say(
        "create_decoded 4096 x 4096 refused before image allocation",
        peak,
        &format!(
            "at most {} and far below the 64 MiB image",
            envelope.intermediate()
        ),
    );

    let ((), negative_control) = measure(|| {
        let image = Image::decode(bytes.as_slice()).expect("decode before trying to reserve");
        assert!(
            service
                .reserve_pixels(image.width(), image.height())
                .is_err(),
            "the held pixel leaves insufficient CPU budget"
        );
        drop(image);
    });
    assert!(
        negative_control >= MAX_DECODED_BYTES,
        "unreserved negative control peak was only {negative_control}"
    );
    say(
        "negative control without a reservation",
        negative_control,
        "decode allocates the image before the reservation is refused",
    );

    drop(held);
    service.close();
    let _ = service.finish_teardown();
    drop(service);
    fs::remove_dir_all(root).expect("remove scratch assets");
}

fn at_the_decoded_cap() {
    let bytes = the_decoded_cap_file();
    let (image, peak) = measure(|| Image::decode(bytes.as_slice()).expect("a decode"));
    assert_eq!(
        (image.width(), image.height(), image.pixels().len()),
        (4096, 4096, MAX_DECODED_BYTES)
    );
    // The first byte is 0xF0: four white pixels, then four black.
    assert_eq!(&image.pixels()[..16], &[255; 16]);
    assert_eq!(&image.pixels()[16..32], &[0, 0, 0, 255].repeat(4));
    let envelope = Envelope::of(4096, 4096, 512);
    assert!(envelope.contains(peak), "peak {peak} outside {envelope:?}");
    assert!(peak >= MAX_DECODED_BYTES, "the image itself: {peak}");
    say(
        "4096 x 4096 at the decoded cap",
        peak,
        &format!("envelope {}", envelope.total()),
    );
}

/// A 16-bit interlaced RGBA image of `side` x `side` pixels, built here.
fn rgba16_interlaced(side: u16) -> Vec<u8> {
    let rows = png::scanlines(
        usize::from(side),
        usize::from(side),
        8,
        true,
        |x, y, out| {
            let (x, y) = (u16::try_from(x).expect("x"), u16::try_from(y).expect("y"));
            let samples = [
                ((x % 251) << 8) | (x / 251),
                ((y % 241) << 8) | (y / 241),
                0x8001,
                0xFFFF,
            ];
            for (slot, sample) in out.chunks_exact_mut(2).zip(samples) {
                slot.copy_from_slice(&sample.to_be_bytes());
            }
        },
    );
    png::file(
        &png::ihdr(u32::from(side), u32::from(side), 16, RGBA, 1),
        &[],
        &rows,
    )
}

/// A 16-bit interlaced RGBA image whose whole raw frame is larger than everything but the image in the envelope:
/// it fits because no raw frame is allocated, and a decode that allocated one would not.
fn sixteen_bit_interlaced_rgba() {
    let side = 1536_u16;
    let bytes = rgba16_interlaced(side);
    let side = usize::from(side);
    let (image, peak) = measure(|| Image::decode(bytes.as_slice()).expect("a decode"));
    assert_eq!(image.pixels().len(), side * side * 4);
    // Pixel (3, 2) is red 0x0300, green 0x0200, blue 0x8001 and alpha 0xFFFF, which reduce to 3, 2, 128 and 255
    // (16 to 8 bits rounds as (v * 255 + 32767) / 65535).
    let at = (2 * side + 3) * 4;
    assert_eq!(&image.pixels()[at..at + 4], &[3, 2, 128, 255]);
    let envelope = Envelope::of(side, side, side * 8);
    assert!(envelope.contains(peak), "peak {peak} outside {envelope:?}");
    // The size is chosen so that a raw frame is more than the whole non-image part of the envelope: a decode
    // that held one next to the image would be outside it.
    let raw_frame = side * side * 8;
    assert!(
        raw_frame > envelope.intermediate(),
        "raw frame {raw_frame}, intermediate {}",
        envelope.intermediate()
    );
    assert!(
        !envelope.contains(envelope.image + raw_frame),
        "a raw frame and the image together are outside"
    );
    say(
        "16-bit interlaced RGBA, 1536 x 1536",
        peak,
        &format!("envelope {}, raw frame {raw_frame}", envelope.total()),
    );
}

/// The envelope is a real bound: the same decodes measured against envelopes built with injected small
/// allowances, or made to allocate more, fail the check.
fn the_envelope_can_fail() {
    // 1. The same 16-bit decode, against an envelope with no allowance for the library and no slack.
    let side = 512_usize;
    let bytes = rgba16_interlaced(512);
    let (_, peak) = measure(|| Image::decode(bytes.as_slice()).expect("a decode"));
    let real = Envelope::of(side, side, side * 8);
    assert!(real.contains(peak));
    let injected = Envelope {
        library: 0,
        slack: 0,
        ..real
    };
    assert!(
        !injected.contains(peak),
        "peak {peak} fits even {injected:?}"
    );
    // 2. A decode that also holds an allocation bigger than the envelope's non-image part.
    let (_, wasteful) = measure(|| {
        let image = Image::decode(bytes.as_slice()).expect("a decode");
        let hoard = vec![1_u8; real.intermediate() + MIB];
        std::hint::black_box(&hoard);
        image
    });
    assert!(!real.contains(wasteful), "peak {wasteful} fits {real:?}");
    say(
        "512 x 512 against an injected small envelope",
        peak,
        &format!("outside {injected:?}"),
    );
}

/// The read buffer is the one fixed buffer the input goes through: a smaller one shows in the peak.
fn the_read_buffer() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rgba16-adam7.png");
    let bytes = fs::read(path).expect("a fixture");
    let (_, default) = measure(|| Image::decode(bytes.as_slice()).expect("a decode"));
    let small = DecodeLimits {
        read_buffer: 1024,
        ..DecodeLimits::SHIPPED
    };
    let (_, shrunk) = measure(|| Image::decode_with(bytes.as_slice(), &small).expect("a decode"));
    assert!(default >= READ_BUFFER_BYTES, "default {default}");
    assert!(
        default - shrunk >= READ_BUFFER_BYTES - 2048,
        "{default} against {shrunk}"
    );
    say(
        "read buffer 64 KiB against 1 KiB",
        default,
        &format!("{shrunk} with 1 KiB"),
    );
}

fn main() {
    refused_by_the_header_alone();
    over_the_encoded_cap();
    large_metadata_under_the_cap();
    at_the_decoded_cap();
    a_refused_reservation_allocates_nothing_image_sized();
    create_decoded_reserves_before_image_allocation();
    sixteen_bit_interlaced_rgba();
    the_envelope_can_fail();
    the_read_buffer();
    println!("allocation: all scenarios passed");
}
