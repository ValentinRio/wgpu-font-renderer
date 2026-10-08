#![cfg(not(target_arch = "wasm32"))]

pub fn gpu() -> Option<(wgpu::Device, wgpu::Queue, wgpu::SurfaceConfiguration)> {
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
