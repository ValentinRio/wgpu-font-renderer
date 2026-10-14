#![cfg(not(target_arch = "wasm32"))]

#[allow(dead_code)]
pub mod reference;
#[allow(dead_code)]
pub mod centre;
#[allow(dead_code)]
pub mod coverage_limits;

pub fn gpu() -> Option<(wgpu::Device, wgpu::Queue, wgpu::SurfaceConfiguration)> {
    gpu_with_lcd(true)
}

pub fn gpu_with_lcd(enable_lcd: bool) -> Option<(wgpu::Device, wgpu::Queue, wgpu::SurfaceConfiguration)> {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let Ok(adapter) = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
        else {
            assert!(
                std::env::var("REQUIRE_GPU").as_deref() != Ok("1"),
                "REQUIRE_GPU=1 but no GPU adapter is available"
            );
            eprintln!("Skipping GPU test: no GPU adapter available");
            return None;
        };
        eprintln!("GPU test adapter: {}", adapter.get_info().name);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_limits: wgpu::Limits::downlevel_defaults(),
                required_features: if enable_lcd {
                    adapter.features() & wgpu::Features::DUAL_SOURCE_BLENDING
                } else {
                    wgpu::Features::empty()
                },
                ..Default::default()
            })
            .await
            .expect("Request device");
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: wgpu::TextureFormat::Rgba8Unorm,
            color_space: wgpu::SurfaceColorSpace::Srgb,
            width: 128,
            height: 128,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        Some((device, queue, config))
    })
}

// Preserve Cantarell's outlines and positioning tables, but expose only the
// decomposed sequence so swash cannot normalize it to a precomposed e-acute.
#[allow(dead_code)]
pub fn decomposed_font() -> Vec<u8> {
    let mut bytes = include_bytes!("../../fonts/Cantarell-VF.otf").to_vec();
    let font = swash::FontRef::from_index(&bytes, 0).unwrap();
    let mappings = [('e', font.charmap().map('e')), ('\u{301}', font.charmap().map('\u{301}'))];
    assert!(mappings.iter().all(|&(_, id)| id != 0));
    let mut cmap = Vec::new();
    for value in [0u16, 1, 3, 10] { cmap.extend_from_slice(&value.to_be_bytes()); }
    cmap.extend_from_slice(&12u32.to_be_bytes());
    for value in [12u16, 0] { cmap.extend_from_slice(&value.to_be_bytes()); }
    for value in [40u32, 0, 2] { cmap.extend_from_slice(&value.to_be_bytes()); }
    for (ch, id) in mappings {
        for value in [ch as u32, ch as u32, id as u32] {
            cmap.extend_from_slice(&value.to_be_bytes());
        }
    }
    let count = u16::from_be_bytes(bytes[4..6].try_into().unwrap()) as usize;
    let record = (0..count).map(|i| 12 + i * 16)
        .find(|&i| &bytes[i..i + 4] == b"cmap").unwrap();
    bytes.resize(bytes.len().next_multiple_of(4), 0);
    let offset = bytes.len() as u32;
    bytes[record + 8..record + 12].copy_from_slice(&offset.to_be_bytes());
    bytes[record + 12..record + 16].copy_from_slice(&(cmap.len() as u32).to_be_bytes());
    bytes.extend(cmap);
    bytes
}

// Cantarell's MVAR has x-height, strikeout and sub/superscript offsets but no
// ascender record. Reuse its variation store with sorted HASC, HDSC and HLGP
// records and visible weight deltas to exercise all line metrics independently.
#[allow(dead_code)]
pub fn mvar_anchor_font() -> Vec<u8> {
    let mut bytes = include_bytes!("../../fonts/Cantarell-VF.otf").to_vec();
    let count = u16::from_be_bytes(bytes[4..6].try_into().unwrap()) as usize;
    let record = (0..count).map(|i| 12 + i * 16)
        .find(|&i| &bytes[i..i + 4] == b"MVAR").unwrap();
    let offset = u32::from_be_bytes(bytes[record + 8..record + 12].try_into().unwrap()) as usize;
    let length = u32::from_be_bytes(bytes[record + 12..record + 16].try_into().unwrap()) as usize;
    assert_eq!(&bytes[offset + 12..offset + 16], b"sbyo");
    assert_eq!(&bytes[offset + 16..offset + 20], &[0, 0, 0, 3]);
    bytes[offset + 12..offset + 16].copy_from_slice(b"hasc");
    bytes[offset + 20..offset + 24].copy_from_slice(b"hdsc");
    bytes[offset + 28..offset + 32].copy_from_slice(b"hlgp");
    assert_eq!(length, 90);
    bytes[offset + length - 5] = (-50i8) as u8;
    bytes[offset + length - 3] = 30;
    bytes[offset + length - 1] = 100;
    bytes
}

#[allow(dead_code)]
fn replace_font_table(bytes: &mut Vec<u8>, tag: &[u8; 4], table: &[u8]) {
    let count = u16::from_be_bytes(bytes[4..6].try_into().unwrap()) as usize;
    let record = (0..count).map(|i| 12 + i * 16)
        .find(|&i| &bytes[i..i + 4] == tag).unwrap();
    bytes.resize(bytes.len().next_multiple_of(4), 0);
    let offset = bytes.len() as u32;
    bytes[record + 8..record + 12].copy_from_slice(&offset.to_be_bytes());
    bytes[record + 12..record + 16].copy_from_slice(&(table.len() as u32).to_be_bytes());
    bytes.extend_from_slice(table);
}

// A tiny GSUB 1.1 table: liga has no default lookups, but a FeatureVariations
// condition covering normalized zero enables a single substitution I -> H.
#[allow(dead_code)]
pub fn zero_feature_variation_font() -> Vec<u8> {
    let mut bytes = include_bytes!("../../fonts/Cantarell-VF.otf").to_vec();
    let font = swash::FontRef::from_index(&bytes, 0).unwrap();
    let (from, to) = (font.charmap().map('I'), font.charmap().map('H'));
    let u16s = |values: &[u16]| values.iter().flat_map(|v| v.to_be_bytes()).collect::<Vec<_>>();
    let u32s = |values: &[u32]| values.iter().flat_map(|v| v.to_be_bytes()).collect::<Vec<_>>();
    let mut table = u16s(&[1, 1, 14, 34, 46]);
    table.extend(u32s(&[72]));
    table.extend(u16s(&[1]));
    table.extend(b"latn");
    table.extend(u16s(&[8, 4, 0, 0, 0xffff, 1, 0]));
    assert_eq!(table.len(), 34);
    table.extend(u16s(&[1]));
    table.extend(b"liga");
    table.extend(u16s(&[8, 0, 0]));
    assert_eq!(table.len(), 46);
    table.extend(u16s(&[1, 4, 1, 0, 1, 8, 2, 8, 1, to, 1, 1, from]));
    assert_eq!(table.len(), 72);
    table.extend(u16s(&[1, 0]));
    table.extend(u32s(&[1, 16, 30]));
    table.extend(u16s(&[1]));
    table.extend(u32s(&[6]));
    table.extend(u16s(&[1, 0, 0, 0]));
    table.extend(u16s(&[1, 0, 1, 0]));
    table.extend(u32s(&[12]));
    table.extend(u16s(&[0, 1, 0]));
    replace_font_table(&mut bytes, b"GSUB", &table);
    bytes
}

// Add an independent width axis to Cantarell's metadata. Its outlines still
// depend only on weight; this lets chained overrides prove weight inheritance.
#[allow(dead_code)]
pub fn two_axis_font() -> Vec<u8> {
    use skrifa::{MetadataProvider, raw::TableProvider};
    let mut bytes = include_bytes!("../../fonts/Cantarell-VF.otf").to_vec();
    let face = skrifa::FontRef::new(&bytes).unwrap();
    let axis = face.axes().iter().next().unwrap();
    let mut fvar = Vec::new();
    for value in [1u16, 0, 16, 2, 2, 20, 0, 12] { fvar.extend(value.to_be_bytes()); }
    for (tag, min, default, max) in [
        (axis.tag().to_be_bytes(), axis.min_value(), axis.default_value(), axis.max_value()),
        (*b"wdth", 50., 100., 200.),
    ] {
        fvar.extend(tag);
        for value in [min, default, max] { fvar.extend(((value as i32) << 16).to_be_bytes()); }
        for value in [0u16, 256] { fvar.extend(value.to_be_bytes()); }
    }
    let mut avar = face.avar().unwrap().offset_data().as_bytes().to_vec();
    avar[6..8].copy_from_slice(&2u16.to_be_bytes());
    for value in [3i16, -16384, -16384, 0, 0, 16384, 16384] { avar.extend(value.to_be_bytes()); }
    replace_font_table(&mut bytes, b"fvar", &fvar);
    replace_font_table(&mut bytes, b"avar", &avar);
    bytes
}

// GSUB liga: nominal glyph zero followed by 'a' becomes visible 'H'. CGJ is
// unmapped in Roboto, so the ligature retains a default-ignorable source marker.
#[allow(dead_code)]
pub fn missing_ignorable_ligature_font() -> Vec<u8> {
    let mut bytes = include_bytes!("../../fonts/Roboto-Regular.ttf").to_vec();
    let font = swash::FontRef::from_index(&bytes, 0).unwrap();
    let charmap = font.charmap();
    assert_eq!(charmap.map('\u{034F}'), 0);
    let (letter, output) = (charmap.map('a'), charmap.map('H'));
    assert_ne!(letter, 0);
    assert_ne!(output, 0);
    let u16s = |values: &[u16]| values.iter().flat_map(|v| v.to_be_bytes()).collect::<Vec<_>>();
    let mut table = u16s(&[1, 0, 10, 30, 44]);
    table.extend(u16s(&[1]));
    table.extend(b"latn");
    table.extend(u16s(&[8, 4, 0, 0, 0xffff, 1, 0]));
    assert_eq!(table.len(), 30);
    table.extend(u16s(&[1]));
    table.extend(b"liga");
    table.extend(u16s(&[8, 0, 1, 0]));
    assert_eq!(table.len(), 44);
    table.extend(u16s(&[1, 4, 4, 0, 1, 8, 1, 18, 1, 8, 1, 4, output, 2, letter, 1, 1, 0]));
    replace_font_table(&mut bytes, b"GSUB", &table);
    bytes
}
