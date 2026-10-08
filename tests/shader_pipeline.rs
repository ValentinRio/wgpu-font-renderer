#![cfg(not(target_arch = "wasm32"))]

use wgpu_font_renderer::{FontStore, TextRenderer};

mod common;

#[test]
fn shader_pipeline_validates() {
    let Some((device, _queue, _config)) = common::gpu() else {
        return;
    };
    pollster::block_on(async {
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        for format in [
            wgpu::TextureFormat::Bgra8UnormSrgb,
            wgpu::TextureFormat::Rgba8UnormSrgb,
        ] {
            let config = wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format,
                color_space: wgpu::SurfaceColorSpace::Srgb,
                width: 800,
                height: 600,
                present_mode: wgpu::PresentMode::Fifo,
                alpha_mode: wgpu::CompositeAlphaMode::Opaque,
                view_formats: vec![],
                desired_maximum_frame_latency: 2,
            };
            let store = FontStore::new(&device, &config);
            let mut renderer = TextRenderer::new(&device, &config, store.atlas());
            renderer.update_uniforms(&device, [1024, 768]);
        }
        let error = scope.pop().await;
        assert!(error.is_none(), "Pipeline validation failed: {error:?}");
    });
}
