use wgpu_font_renderer::{FontStore, TextRenderer, TypeWriter};

use wgpu::{
    CommandEncoderDescriptor, CompositeAlphaMode, DeviceDescriptor, Features, Instance,
    InstanceDescriptor, Limits, LoadOp, Operations, PresentMode,
    RenderPassColorAttachment, RenderPassDescriptor, RequestAdapterOptions, SurfaceConfiguration,
    TextureFormat, TextureUsages, TextureViewDescriptor,
};
use winit::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::EventLoop,
    window::WindowBuilder,
};

use std::sync::Arc;

fn main() {
    pollster::block_on(run());
}

async fn run() {
    // Set up window
    let (width, height) = (800, 600);
    let event_loop = EventLoop::new().unwrap();
    let window = Arc::new(
        WindowBuilder::new()
            .with_inner_size(LogicalSize::new(width as f64, height as f64))
            .with_title("WGPU Font Renderer")
            .build(&event_loop)
            .unwrap(),
    );
    let size = window.inner_size();
    let scale_factor = window.scale_factor();

    // Set up surface
    let instance = Instance::new(InstanceDescriptor::default());
    let adapter = instance
        .request_adapter(&RequestAdapterOptions::default())
        .await
        .unwrap();
    let (device, queue) = adapter
        .request_device(
            &DeviceDescriptor {
                label: None,
                required_features: Features::empty(),
                required_limits: Limits::downlevel_defaults(),
            },
            None,
        )
        .await
        .unwrap();

    let surface = instance
        .create_surface(window.clone())
        .expect("Create surface");
    let swapchain_format = TextureFormat::Bgra8UnormSrgb;
    let mut config = SurfaceConfiguration {
        usage: TextureUsages::RENDER_ATTACHMENT,
        format: swapchain_format,
        width: size.width,
        height: size.height,
        present_mode: PresentMode::Fifo,
        alpha_mode: CompositeAlphaMode::Auto,
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    };
    surface.configure(&device, &config);

    let mut font_store = FontStore::new(&device, &config);
    let cache_preset = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789,;:!ù*^$=)àç_è-('\"é&²<>+°§/.? ";
    let font_key = font_store.load(&device, &queue, "examples/Roboto-Regular.ttf", cache_preset).expect("Couldn't load the font");

    let mut paragraphs = Vec::new();

    let mut type_writer = TypeWriter::new();
    if let Some(paragraph) = type_writer.shape_text(&font_store, font_key, [100., 100.], 72, [0.68, 0.5, 0.12, 1.], "Salut, c'est cool!") {
        paragraphs.push(paragraph);
    }

    let mut text_renderer = TextRenderer::new(&device, &config, font_store.atlas());

    let _physical_width = (width as f64 * scale_factor) as f32;
    let _physical_height = (height as f64 * scale_factor) as f32;

    event_loop
        .run(move |event, target| {
            if let Event::WindowEvent {
                window_id: _,
                event,
            } = event
            {
                match event {
                    WindowEvent::Resized(size) => {
                        config.width = size.width;
                        config.height = size.height;
                        surface.configure(&device, &config);
                        window.request_redraw();
                        text_renderer.update_uniforms(&device, [size.width, size.height]);
                    }
                    WindowEvent::RedrawRequested => {
                        
                        // Prepare should happen here
                        text_renderer.prepare(&device, &paragraphs, &font_store);
                        
                        let frame = surface.get_current_texture().unwrap();
                        let view = frame.texture.create_view(&TextureViewDescriptor::default());
                        let mut encoder = device
                            .create_command_encoder(&CommandEncoderDescriptor { label: None });
                        {
                            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                                label: None,
                                color_attachments: &[Some(RenderPassColorAttachment {
                                    view: &view,
                                    resolve_target: None,
                                    ops: Operations {
                                        load: LoadOp::Clear(wgpu::Color::WHITE),
                                        store: wgpu::StoreOp::Store,
                                    },
                                })],
                                depth_stencil_attachment: None,
                                timestamp_writes: None,
                                occlusion_query_set: None,
                            });

                            // Render should happen here
                            text_renderer.render(&mut pass, [config.width, config.height]);
                        }

                        queue.submit(Some(encoder.finish()));
                        frame.present();
                    }
                    WindowEvent::CloseRequested => target.exit(),
                    _ => {}
                }
            }
        })
        .unwrap();
}