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
