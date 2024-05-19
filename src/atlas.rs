pub mod allocator;
pub mod allocation;
pub mod layer;

use layer::Layer;
use wgpu::{SurfaceConfiguration, TextureFormat};

use self::{allocation::Allocation, allocator::Allocator};

pub struct Atlas {
    texture: wgpu::Texture,
    texture_view: wgpu::TextureView,
    layers: Vec<Layer>,
    pub texture_format: wgpu::TextureFormat,
}

pub const SIZE: u32 = 2048;

impl Atlas {
    pub fn new(device: &wgpu::Device, surface_config: &SurfaceConfiguration) -> Self {
        
        let extent = wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 2,
        };

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Atlas Texture"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TextureFormat::R32Float,
            usage: wgpu::TextureUsages::COPY_DST
                 | wgpu::TextureUsages::COPY_SRC
                 | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[TextureFormat::R32Float],
        });

        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });

        Self {
            texture,
            texture_view,
            layers: vec![Layer::Empty],
            texture_format: TextureFormat::R32Float,
        }
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.texture_view
    }

    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }

    fn allocate(&mut self, width: u32) -> Option<Allocation> {
        for (i, layer) in self.layers.iter_mut().enumerate() {
            match layer {
                Layer::Empty => {
                    let mut allocator = Allocator::new(SIZE);

                    if let Some(region) = allocator.allocate(width) {
                        *layer = Layer::Busy(allocator);

                        return Some(Allocation {
                            region,
                            layer: i,
                        });
                    }
                }
                Layer::Busy(allocator) => {
                    if let Some(region) = allocator.allocate(width) {
                        return Some(Allocation {
                            region,
                            layer: i,
                        })
                    }
                }
            }
        }

        let mut allocator = Allocator::new(SIZE);

        if let Some(region) = allocator.allocate(width) {
            self.layers.push(Layer::Busy(allocator));

            return Some(Allocation {
                region,
                layer: self.layers.len() - 1,
            });
        }

        None
    }

    pub fn upload(
        &mut self,
        size: u32,
        data: &[u8],
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
    ) -> Option<Allocation> {
        // use wgpu::util::DeviceExt;

        let current_size = self.layers.len();
        let allocation = self.allocate(size)?;

        let new_layers = self.layers.len() - current_size;

        self.grow(new_layers, device, encoder);

        self.upload_allocation(&data, &allocation, queue);

        Some(allocation)
    }

    fn upload_allocation(
        &mut self,
        data: &[u8],
        allocation: &Allocation,
        queue: &wgpu::Queue,
    ) {
        let [x, y] = allocation.position();
        let size = allocation.size();
        let layer = allocation.layer();

        let mut blocks: Vec<[u32; 5]> = Vec::new();

        let first_line = SIZE - x;

        if size < first_line {
            blocks.push([x, y, size, size, 1]);
        } else {
            let nb_lines = f32::ceil((size as f32 - first_line as f32) / SIZE as f32);

            let last_line = (size as f32 - first_line as f32) % SIZE as f32;

            blocks.push([x, y, first_line, first_line, 1]);

            if nb_lines > 1. {
                blocks.push([0, y + 1, size - first_line - last_line as u32, SIZE, nb_lines as u32]);
            }

            if last_line != 0. {
                blocks.push([0, y + f32::max(nb_lines, 1.) as u32, last_line as u32, last_line as u32, 1]);
            }
        }

        let mut offset = 0;

        blocks.iter().for_each(|[x, y, size, width, height]| {
            let extent = wgpu::Extent3d {
                width: *width,
                height: *height,
                depth_or_array_layers: 1,
            };

            let byte_size = *size as usize * 4;

            let data_slice = &data[offset..offset + byte_size];

            queue.write_texture(wgpu::ImageCopyTexture {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: *x,
                    y: *y,
                    z: layer as u32,
                },
                aspect: wgpu::TextureAspect::default()
            }, data_slice, wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(*height),
            }, extent);

            offset += byte_size;
        });
    }

    fn grow(
        &mut self,
        amount: usize,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        if amount == 0 {
            return;
        }

        let new_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Atlas Texture"),
            size: wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: self.layers.len() as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TextureFormat::R32Float,
            usage: wgpu::TextureUsages::COPY_DST
                 | wgpu::TextureUsages::COPY_SRC
                 | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[TextureFormat::R32Float],
        });

        let layers_to_copy = self.layers.len() - amount;

        for (i, layer) in self.layers.iter_mut().take(layers_to_copy).enumerate() {
            if layer.is_empty() {
                continue;
            }

            encoder.copy_texture_to_texture(
                wgpu::ImageCopyTexture {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: i as u32,
                    },
                    aspect: wgpu::TextureAspect::default()
                },
                wgpu::ImageCopyTexture {
                    texture: &new_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: i as u32,
                    },
                    aspect: wgpu::TextureAspect::default()
                },
                wgpu::Extent3d {
                    width: SIZE,
                    height: SIZE,
                    depth_or_array_layers: 1,
                }
            )
        }

        self.texture = new_texture;
        self.texture_view = self.texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
    }
}