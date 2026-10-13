#![cfg(not(target_arch = "wasm32"))]

use wgpu_font_renderer::{FontStore, SubpixelOrder, TextRenderer, TypeWriter};
mod common;
#[path = "common/draw.rs"]
mod draw;

fn check_alpha(device: &wgpu::Device, queue: &wgpu::Queue, order: SubpixelOrder) {
    let mut store = FontStore::new(device);
    let text = "HiB@";
    let key = store.load_from_bytes(device, queue, include_bytes!("../fonts/Roboto-Regular.ttf"), text).unwrap();
    let mut renderer = TextRenderer::with_subpixel_order(device, wgpu::TextureFormat::Rgba8Unorm, &store, order);
    let mut writer = TypeWriter::new();
    // Black text on white isolates coverage from rounding of two coloured blend terms.
    let background = wgpu::Color::WHITE;
    let mut renders = Vec::new();
    for alpha in [0., 1., 0.5] {
        let paragraph = writer.shape_text(&store, key, [12.25, 24.5], 24., [0., 0., 0., alpha], text).unwrap();
        renderer.prepare(device, queue, &[paragraph], &store);
        renders.push(draw::draw_pixels(device, queue, &mut renderer, [128, 128], background));
    }
    assert!(renders[1].iter().zip(&renders[0]).any(|(a, b)| a != b), "full alpha must draw ink");
    for ((zero, full), half) in renders[0].iter().zip(&renders[1]).zip(&renders[2]) {
        let contribution = f32::from(*full) - f32::from(*zero);
        let half_contribution = f32::from(*half) - f32::from(*zero);
        assert!((half_contribution - contribution * 0.5).abs() <= 1., "alpha scaling: {zero}, {full}, {half}");
    }
    for pixel in renders[0].as_chunks::<4>().0 {
        assert_eq!(*pixel, [255; 4], "zero alpha must leave the background unchanged");
    }
}

#[test]
fn grayscale_alpha_scales_pixel_contributions() {
    let Some((device, queue, _)) = common::gpu() else { return; };
    check_alpha(&device, &queue, SubpixelOrder::None);
}

#[test]
fn lcd_alpha_scales_pixel_contributions() {
    let Some((device, queue, _)) = common::gpu() else { return; };
    if !device.features().contains(wgpu::Features::DUAL_SOURCE_BLENDING) {
        eprintln!("Skipping LCD alpha test: adapter lacks DUAL_SOURCE_BLENDING");
        return;
    }
    check_alpha(&device, &queue, SubpixelOrder::Rgb);
}

#[test]
fn fallback_alpha_scales_pixel_contributions() {
    let Some((device, queue, _)) = common::gpu_with_lcd(false) else { return; };
    check_alpha(&device, &queue, SubpixelOrder::Rgb);
}
