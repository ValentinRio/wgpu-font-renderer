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
// ascender record. Reuse its variation store with a sorted HASC record and a
// visible positive-weight delta to exercise baseline placement independently.
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
    assert_eq!(length, 90);
    bytes[offset + length - 1] = 100;
    bytes
}
