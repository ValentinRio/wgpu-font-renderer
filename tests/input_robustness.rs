#![cfg(not(target_arch = "wasm32"))]

mod common;

use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    time::Instant,
};
use wgpu_font_renderer::{internals, FontStore, TypeWriter};

const SEED: u64 = 0x4f6e_746c_696e_6573;
const CASES: usize = 10_000;
const GPU_CASES: usize = 300;
const FONTS: [&[u8]; 3] = [
    include_bytes!("../fonts/Roboto-Regular.ttf"),
    include_bytes!("../fonts/Cantarell-VF.otf"),
    include_bytes!("../fonts/NotoSerifSC-Subset.otf"),
];
const PRESET: &str = "AgB@éÅß龍鬱齉直接\0\n\t\u{301} العربية אבג\u{ffff}\u{10ffff}";

struct Random(u64);
impl Random {
    fn case(seed: u64, case: usize) -> Self {
        Self(seed.wrapping_add((case as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15)))
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, limit: usize) -> usize {
        (self.next() % limit as u64) as usize
    }
}

// Every case has its own generator, so replay does not depend on earlier cases.
fn input(seed: u64, case: usize) -> Vec<u8> {
    let mut random = Random::case(seed, case);
    if case.is_multiple_of(5) {
        let lengths = [0, 1, 4, 12, 64, 256, 1024, 4096];
        let len = random.below(lengths[(case / 5) % lengths.len()] + 1);
        return (0..len).map(|_| random.next() as u8).collect();
    }
    let mut bytes = FONTS[(case / 5) % FONTS.len()].to_vec();
    let count = u16::from_be_bytes(bytes[4..6].try_into().unwrap()) as usize;
    match case % 5 {
        1 => {
            for _ in 0..1 + random.below(16) {
                let position = random.below(bytes.len());
                bytes[position] ^= 1 << random.below(8);
            }
        }
        2 => {
            let len = random.below(bytes.len());
            bytes.truncate(len);
        }
        3 => {
            let record = 12 + random.below(count) * 16;
            // Deliberately overwrite complete offsets AND lengths, including
            // overflow and past-end values; random byte flips rarely reach these.
            for field in [8, 12] {
                let value = match random.below(4) {
                    0 => 0,
                    1 => u32::MAX,
                    2 => bytes.len() as u32 + 1,
                    _ => random.next() as u32,
                };
                bytes[record + field..record + field + 4].copy_from_slice(&value.to_be_bytes());
            }
        }
        _ => {
            let record = 12 + random.below(count) * 16;
            let start =
                u32::from_be_bytes(bytes[record + 8..record + 12].try_into().unwrap()) as usize;
            let len =
                u32::from_be_bytes(bytes[record + 12..record + 16].try_into().unwrap()) as usize;
            if len != 0 {
                let position = start + random.below(len);
                let end = (position + 1 + random.below(32)).min(start + len);
                bytes[position..end].fill(random.next() as u8);
            }
        }
    }
    bytes
}

fn text(seed: u64, case: usize) -> String {
    let mut random = Random::case(seed ^ 0x554e_4943_4f44_4521, case);
    if case.is_multiple_of(1000) {
        // Include long single-script runs and combining/control-only clusters.
        return ["A", "\u{301}", "\u{e000}", "\0", "龍"][(case / 1000) % 5].repeat(4096);
    }
    let lengths = [0, 1, 2, 8, 16, 32, 64];
    let len = random.below(lengths[case % lengths.len()] + 1);
    (0..len)
        .map(|_| {
            let codepoint = match random.below(6) {
                0 => random.below(0x20) as u32,
                1 => 0x300 + random.below(0x70) as u32,
                2 => [0x378, 0x379, 0x380, 0xffff, 0x10ffff][random.below(5)],
                3 => 0xe000 + random.below(0x1900) as u32,
                _ => random.below(0x110000) as u32,
            };
            // Map the surrogate interval to scalars rather than creating invalid UTF-8.
            char::from_u32(codepoint).unwrap_or('\u{fffd}')
        })
        .collect()
}

fn no_panic<T>(seed: u64, case: usize, path: &str, action: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(action)).unwrap_or_else(|panic| {
        eprintln!("{path}: seed=0x{seed:016x}, case={case}; replay with ROBUSTNESS_SEED={seed:x} ROBUSTNESS_CASE={case}");
        std::panic::resume_unwind(panic)
    })
}

#[test]
fn seeded_loader_and_shaper_never_panic() {
    let start = Instant::now();
    let seed = std::env::var("ROBUSTNESS_SEED")
        .map(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).unwrap())
        .unwrap_or(SEED);
    let replay = std::env::var("ROBUSTNESS_CASE")
        .ok()
        .map(|s| s.parse::<usize>().unwrap());
    let cases: Vec<_> = replay.map_or_else(|| (0..CASES).collect(), |case| vec![case]);
    let (mut accepted, mut prepared) = (0, 0);
    for &case in &cases {
        let bytes = input(seed, case);
        let result = no_panic(seed, case, "CPU loader", || {
            internals::check_font_bytes(&bytes, PRESET)
        });
        // Case 8941 once overflowed i16 glyph-height arithmetic; it now only has to not panic.
        if let Ok(glyphs) = result {
            accepted += 1;
            if glyphs > 0 {
                prepared += 1;
            }
        }
    }
    eprintln!(
        "CPU loader: {} inputs, {accepted} accepted, {prepared} prepared outlined glyphs, {:?}",
        cases.len(),
        start.elapsed()
    );
    if replay.is_none() {
        assert!(prepared >= 3_000, "must exercise glyph-cache construction, not only header checks");
    }

    let mut writer = TypeWriter::new();
    let Some((device, queue, _)) = common::gpu() else {
        // Exercise the same shaping implementation even without a GPU atlas.
        for (font, bytes) in FONTS.iter().enumerate() {
            for &case in &cases {
                let text = text(seed, case);
                no_panic(seed, case, &format!("CPU shaping font {font}"), || {
                    internals::shape_font_bytes(&mut writer, bytes, &text)
                });
            }
        }
        eprintln!("CPU shaping: {} strings per font, {} calls; GPU loader and shape_text: 0 (no adapter); total {:?}", cases.len(), cases.len() * FONTS.len(), start.elapsed());
        return;
    };
    let gpu_cases = replay.map_or_else(
        || {
            (0..GPU_CASES)
                .chain(std::iter::once(8941))
                .collect::<Vec<_>>()
        },
        |case| vec![case],
    );
    let mut store = FontStore::new(&device);
    let mut gpu_accepted = 0;
    for &case in &gpu_cases {
        if case.is_multiple_of(25) {
            store = FontStore::new(&device);
        }
        let bytes = input(seed, case);
        let result = no_panic(seed, case, "GPU load_from_bytes", || {
            store.load_from_bytes(&device, &queue, &bytes, PRESET)
        });
        if result.is_ok() {
            gpu_accepted += 1;
        }
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    }
    eprintln!(
        "GPU load_from_bytes: {} inputs, {gpu_accepted} accepted",
        gpu_cases.len()
    );
    if replay.is_none() {
        assert!(gpu_accepted >= 100, "GPU path must load real fonts, not only reject");
    }
    let mut store = FontStore::new(&device);
    for (font, bytes) in FONTS.iter().enumerate() {
        let key = store
            .load_from_bytes(&device, &queue, bytes, PRESET)
            .unwrap();
        for &case in &cases {
            let text = text(seed, case);
            no_panic(seed, case, &format!("shape_text font {font}"), || {
                assert!(writer
                    .shape_text(&store, key, [0.; 2], 32., [1.; 4], &text)
                    .is_some());
            });
        }
    }
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    eprintln!(
        "shape_text: {} strings per font, {} calls; total {:?}",
        cases.len(),
        cases.len() * FONTS.len(),
        start.elapsed()
    );
}

// Replace Roboto's outlines with `glyphs` at ids 0.., every other id empty.
// New long-format glyf and loca tables are appended and the directory repointed;
// parsers here do not verify checksums.
fn with_glyphs(glyphs: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = FONTS[0].to_vec();
    let read16 = |b: &[u8], at: usize| u16::from_be_bytes([b[at], b[at + 1]]) as usize;
    let read32 = |b: &[u8], at: usize| u32::from_be_bytes(b[at..at + 4].try_into().unwrap()) as usize;
    let record = |b: &[u8], tag: &[u8; 4]| {
        (0..read16(b, 4)).map(|i| 12 + i * 16).find(|&r| &b[r..r + 4] == tag).unwrap()
    };
    let glyph_count = read16(&bytes, read32(&bytes, record(&bytes, b"maxp") + 8) + 4);
    let mut glyf = Vec::new();
    let mut loca = Vec::new();
    for id in 0..=glyph_count {
        loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());
        if let Some(glyph) = glyphs.get(id) {
            glyf.extend_from_slice(glyph);
            glyf.resize(glyf.len().next_multiple_of(4), 0);
        }
    }
    // Real fonts keep hmtx's left side bearing equal to the glyph's x_min, and
    // skrifa shifts outlines to it, so match it for the crafted glyphs.
    let hmtx = read32(&bytes, record(&bytes, b"hmtx") + 8);
    let metrics = read16(&bytes, read32(&bytes, record(&bytes, b"hhea") + 8) + 34);
    for (id, glyph) in glyphs.iter().enumerate().take(metrics) {
        bytes[hmtx + id * 4 + 2..hmtx + id * 4 + 4].copy_from_slice(&glyph[2..4]);
    }
    let head = read32(&bytes, record(&bytes, b"head") + 8);
    bytes[head + 50..head + 52].copy_from_slice(&1u16.to_be_bytes());
    for (tag, table) in [(b"glyf", glyf), (b"loca", loca)] {
        bytes.resize(bytes.len().next_multiple_of(4), 0);
        let r = record(&bytes, tag);
        let offset = bytes.len() as u32;
        bytes[r + 8..r + 12].copy_from_slice(&offset.to_be_bytes());
        bytes[r + 12..r + 16].copy_from_slice(&(table.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&table);
    }
    bytes
}

fn glyph_header(contours: i16, bbox: [i16; 4]) -> Vec<u8> {
    [contours].iter().chain(&bbox).flat_map(|v| v.to_be_bytes()).collect()
}

// One closed contour of `points` on-curve points stepping right by `step`
// units from `x_min`, with y alternating 0 and 1, so it has `points` segments.
fn simple_glyph(points: u16, x_min: i16, step: u8) -> Vec<u8> {
    let x_max = (i32::from(x_min) + (i32::from(points) - 1) * i32::from(step)) as i16;
    let mut glyph = glyph_header(1, [x_min, 0, x_max, 1]);
    glyph.extend_from_slice(&(points - 1).to_be_bytes());
    glyph.extend_from_slice(&0u16.to_be_bytes());
    // Flags: on curve; the first point has word x/y, the rest have a short
    // positive x and a short y that is +1 on odd points and -1 on even ones.
    glyph.push(0x01);
    glyph.extend((1..points).map(|i| 0x01 | 0x02 | 0x10 | 0x04 | if i % 2 == 1 { 0x20 } else { 0 }));
    glyph.extend_from_slice(&x_min.to_be_bytes());
    glyph.extend(std::iter::repeat_n(step, points as usize - 1));
    glyph.extend_from_slice(&0i16.to_be_bytes());
    glyph.extend(std::iter::repeat_n(1u8, points as usize - 1));
    glyph
}

// A composite referencing `component` `copies` times at zero offset.
fn composite_glyph(component: u16, copies: usize) -> Vec<u8> {
    let mut glyph = glyph_header(-1, [0, 0, 1, 1]);
    for i in 0..copies {
        let more = if i + 1 < copies { 0x0020 } else { 0 };
        glyph.extend_from_slice(&(0x0001u16 | 0x0002 | more).to_be_bytes());
        glyph.extend_from_slice(&component.to_be_bytes());
        glyph.extend_from_slice(&[0; 4]);
    }
    glyph
}

fn too_complex(result: Result<usize, wgpu_font_renderer::LoadingError>) -> bool {
    matches!(result, Err(wgpu_font_renderer::LoadingError::GlyphTooComplex(_)))
}

#[test]
fn glyph_wider_than_i16_loads() {
    // OpenType allows x_min=-20000 and x_max=19750; the span exceeds i16::MAX.
    let font = with_glyphs(&[simple_glyph(160, -20000, 250)]);
    assert_eq!(internals::check_font_bytes(&font, "").unwrap(), 1);
    if let Some((device, queue, _)) = common::gpu() {
        let mut store = FontStore::new(&device);
        let id = store.load_from_bytes(&device, &queue, &font, "").unwrap();
        let glyph = &internals::font(&store, id).unwrap().glyph_cache[&0];
        assert_eq!(glyph.bbox.width(), 39_750);
    }
}

#[test]
fn composite_fan_out_is_rejected_before_drawing() {
    // Twenty levels, each referencing the next twice: 2^20 references to an
    // empty leaf. Nothing would ever be drawn, so only the reference budget
    // can reject it; with a large leaf skrifa would size scratch memory for
    // 2^20 copies of its points before drawing.
    let glyphs: Vec<_> = (1..=20).map(|next| composite_glyph(next, 2)).collect();
    assert!(too_complex(internals::check_font_bytes(&with_glyphs(&glyphs), "")));
}

#[test]
fn expanded_point_count_is_bounded() {
    // Two references to a 40,000-point glyph expand to 80,000 points.
    let font = with_glyphs(&[composite_glyph(1, 2), simple_glyph(40_000, 0, 0)]);
    assert!(too_complex(internals::check_font_bytes(&font, "")));
    // One reference stays within the point budget and fails on segments instead.
    let font = with_glyphs(&[composite_glyph(1, 1), simple_glyph(40_000, 0, 0)]);
    assert!(too_complex(internals::check_font_bytes(&font, "")));
}

#[test]
fn encoded_segments_are_bounded() {
    let font = with_glyphs(&[simple_glyph(16_384, 0, 0)]);
    assert_eq!(internals::check_font_bytes(&font, "").unwrap(), 1);
    // 16,386 points: 16,385 lines plus the closing line back to the start.
    let font = with_glyphs(&[simple_glyph(16_386, 0, 0)]);
    assert!(too_complex(internals::check_font_bytes(&font, "")));
}
