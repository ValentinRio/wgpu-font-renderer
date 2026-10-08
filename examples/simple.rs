use wgpu_font_renderer::{FontStore, TextRenderer, TypeWriter};

use wgpu::{
    CommandEncoderDescriptor, CurrentSurfaceTexture, DeviceDescriptor, Instance,
    InstanceDescriptor, Limits, LoadOp, Operations, PresentMode, RenderPassColorAttachment,
    RenderPassDescriptor, RequestAdapterOptions, SurfaceConfiguration, TextureUsages,
    TextureViewDescriptor,
};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy},
    window::{Window, WindowId},
};

use std::sync::Arc;

struct State {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: SurfaceConfiguration,
    text_renderer: TextRenderer,
}

impl State {
    async fn new(window: Arc<Window>) -> Self {
        let instance = Instance::new(InstanceDescriptor {
            #[cfg(target_arch = "wasm32")]
            backends: wgpu::Backends::BROWSER_WEBGPU,
            ..InstanceDescriptor::new_with_display_handle(Box::new(window.clone()))
        });
        let surface = instance
            .create_surface(window.clone())
            .expect("Create surface");
        let adapter = instance
            .request_adapter(&RequestAdapterOptions {
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .expect("Request adapter");
        let (device, queue) = adapter
            .request_device(&DeviceDescriptor {
                required_limits: Limits::downlevel_defaults(),
                ..Default::default()
            })
            .await
            .expect("Request device");
        device.on_uncaptured_error(Arc::new(|error| panic!("{error}")));

        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| format.is_srgb())
            .unwrap_or(capabilities.formats[0]);
        let size = window.inner_size();
        let view_format = format.add_srgb_suffix();
        let config = SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Srgb,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: PresentMode::Fifo,
            alpha_mode: capabilities.alpha_modes[0],
            view_formats: if view_format != format {
                vec![view_format]
            } else {
                vec![]
            },
            desired_maximum_frame_latency: 2,
        };

        let renderer_config = SurfaceConfiguration {
            format: view_format,
            ..config.clone()
        };
        let mut font_store = FontStore::new(&device, &renderer_config);
        let cache_preset = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789,;:!ù*^$=)àç_è-('\"é&²<>+°§/.? ";
        let font_key = font_store
            .load_from_bytes(
                &device,
                &queue,
                include_bytes!("Roboto-Regular.ttf"),
                cache_preset,
            )
            .expect("Couldn't load the font");

        let mut paragraphs = Vec::new();
        let mut type_writer = TypeWriter::new();
        if let Some(paragraph) = type_writer.shape_text(
            &font_store,
            font_key,
            [100., 100.],
            72,
            [0.68, 0.5, 0.12, 1.],
            "Salut, c'est cool!",
        ) {
            paragraphs.push(paragraph);
        }
        let mut text_renderer = TextRenderer::new(&device, &renderer_config, font_store.atlas());
        text_renderer.prepare(&device, &paragraphs, &font_store);

        Self {
            window,
            surface,
            device,
            queue,
            config,
            text_renderer,
        }
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        self.config.width = size.width;
        self.config.height = size.height;
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.surface.configure(&self.device, &self.config);
        self.text_renderer
            .update_uniforms(&self.device, [size.width, size.height]);
        self.window.request_redraw();
    }

    fn redraw(&mut self) {
        if self.config.width == 0 || self.config.height == 0 {
            return;
        }
        let (frame, suboptimal) = match self.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(frame) => (frame, false),
            CurrentSurfaceTexture::Suboptimal(frame) => (frame, true),
            CurrentSurfaceTexture::Outdated => {
                self.resize(self.window.inner_size());
                return;
            }
            CurrentSurfaceTexture::Lost => panic!("Surface lost"),
            CurrentSurfaceTexture::Validation => panic!("Surface validation error"),
            CurrentSurfaceTexture::Timeout | CurrentSurfaceTexture::Occluded => {
                self.window.request_redraw();
                return;
            }
        };
        let view = frame.texture.create_view(&TextureViewDescriptor {
            format: Some(self.config.format.add_srgb_suffix()),
            ..Default::default()
        });
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(wgpu::Color::WHITE),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            self.text_renderer
                .render(&mut pass, [self.config.width, self.config.height]);
        }
        self.queue.submit(Some(encoder.finish()));
        self.queue.present(frame);
        if suboptimal {
            self.resize(self.window.inner_size());
        }
    }
}

struct App {
    proxy: EventLoopProxy<Box<State>>,
    window: Option<Arc<Window>>,
    state: Option<Box<State>>,
}

impl ApplicationHandler<Box<State>> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes().with_title("WGPU Font Renderer");
        #[cfg(not(target_arch = "wasm32"))]
        let attributes = attributes.with_inner_size(winit::dpi::LogicalSize::new(800., 600.));
        #[cfg(target_arch = "wasm32")]
        let attributes = {
            use wasm_bindgen::JsCast;
            use winit::platform::web::WindowAttributesExtWebSys;

            let document = web_sys::window().unwrap().document().unwrap();
            let canvas = document
                .get_element_by_id("canvas")
                .map(|element| element.dyn_into::<web_sys::HtmlCanvasElement>().unwrap());
            attributes.with_canvas(canvas).with_append(true)
        };
        let window = Arc::new(event_loop.create_window(attributes).unwrap());
        self.window = Some(window.clone());
        let proxy = self.proxy.clone();
        #[cfg(not(target_arch = "wasm32"))]
        {
            let state = pollster::block_on(State::new(window));
            let _ = proxy.send_event(Box::new(state));
        }
        #[cfg(target_arch = "wasm32")]
        wasm_bindgen_futures::spawn_local(async move {
            let state = State::new(window).await;
            let _ = proxy.send_event(Box::new(state));
        });
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, mut state: Box<State>) {
        state.resize(state.window.inner_size());
        self.state = Some(state);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        if let WindowEvent::CloseRequested = event {
            event_loop.exit();
            return;
        }
        if let Some(state) = self.state.as_mut() {
            match event {
                WindowEvent::Resized(size) => state.resize(size),
                WindowEvent::RedrawRequested => state.redraw(),
                _ => {}
            }
        }
    }
}

fn main() {
    #[cfg(target_arch = "wasm32")]
    console_error_panic_hook::set_once();

    let event_loop = EventLoop::<Box<State>>::with_user_event().build().unwrap();
    let app = App {
        proxy: event_loop.create_proxy(),
        window: None,
        state: None,
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut app = app;
        event_loop.run_app(&mut app).unwrap();
    }
    #[cfg(target_arch = "wasm32")]
    {
        use winit::platform::web::EventLoopExtWebSys;
        event_loop.spawn_app(app);
    }
}
