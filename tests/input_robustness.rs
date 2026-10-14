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
    let mut accepted = 0;
    for &case in &cases {
        let bytes = input(seed, case);
        let result = no_panic(seed, case, "CPU loader", || {
            internals::check_font_bytes(&bytes, PRESET)
        });
        if seed == SEED && case == 8941 {
            assert!(
                matches!(result, Err(wgpu_font_renderer::LoadingError::InvalidFont)),
                "overflow regression must be rejected"
            );
        }
        if result.is_ok() {
            accepted += 1;
        }
    }
    eprintln!(
        "CPU loader: {} inputs, {accepted} accepted, {:?}",
        cases.len(),
        start.elapsed()
    );
    if replay.is_none() {
        assert!(accepted >= 300, "must exercise glyph-cache construction");
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
        if seed == SEED && case == 8941 {
            assert!(
                matches!(result, Err(wgpu_font_renderer::LoadingError::InvalidFont)),
                "GPU overflow regression must be rejected"
            );
        }
        if result.is_ok() {
            gpu_accepted += 1;
        }
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    }
    eprintln!(
        "GPU load_from_bytes: {} inputs, {gpu_accepted} accepted",
        gpu_cases.len()
    );
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
