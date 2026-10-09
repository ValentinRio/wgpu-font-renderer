/// Linear allocation within a square atlas layer.
pub mod allocator;
/// Atlas region and layer metadata.
pub mod allocation;
/// Empty/busy layer bookkeeping.
pub mod layer;

use layer::Layer;
use wgpu::{SurfaceConfiguration, TextureFormat};

use self::{allocation::Allocation, allocator::Allocator};

/// R32Float texture array storing outlines as row-major float texels.
/// Texel coordinates start at the top left, x rightward and y downward.
pub struct Atlas {
    texture: wgpu::Texture,
    texture_view: wgpu::TextureView,
    layers: Vec<Layer>,
    band_layers: Vec<Layer>,
    /// Storage format of the outline texture, initially R32Float.
    pub texture_format: wgpu::TextureFormat,
}

/// Width and height of each atlas layer in texels; also encoded in the shader.
pub const SIZE: u32 = 2048;

impl Atlas {
    /// Create an empty atlas; the surface configuration is currently unused.
    /// Requires device support for a 2048-square R32Float array texture.
    /// Unsupported resources cause wgpu validation errors.
    pub fn new(device: &wgpu::Device, _surface_config: &SurfaceConfiguration) -> Self {
        
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
            band_layers: vec![Layer::Empty],
            texture_format: TextureFormat::R32Float,
        }
    }

    /// Borrow the current array view; growth replaces this view.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.texture_view
    }

    /// Number of logical texture layers, including outline and band layers.
    pub fn layer_count(&self) -> usize {
        (self.layers.len() * 2 - 1).max(self.band_layers.len() * 2)
    }

    fn allocate(&mut self, width: u32) -> Option<Allocation> {
        let mut allocation = allocate_layer(&mut self.layers, width)?;
        allocation.layer *= 2;
        Some(allocation)
    }

    /// Reserve `size` float texels and upload their native-endian bytes.
    /// Returns `None` if a single layer cannot hold the allocation. During growth,
    /// existing layers are copied and submitted before subsequent queue writes.
    /// Device and queue must match the atlas; layer limits cause GPU errors.
    ///
    /// # Panics
    /// Panics if `data` contains fewer than `size * 4` bytes.
    pub fn upload(
        &mut self,
        size: u32,
        data: &[u8],
        device: &wgpu::Device,
        _encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
    ) -> Option<Allocation> {
        let allocation = self.allocate(size)?;
        self.grow(device, queue);

        self.upload_allocation(&data, &allocation, queue);

        Some(allocation)
    }

    /// Upload bands into odd layers, keeping flat outline positions unchanged.
    /// Has the same byte-count and device requirements as [`Self::upload`].
    pub fn upload_bands(
        &mut self,
        size: u32,
        data: &[u8],
        device: &wgpu::Device,
        _encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
    ) -> Option<Allocation> {
        let mut allocation = allocate_layer(&mut self.band_layers, size)?;
        allocation.layer = allocation.layer * 2 + 1;
        self.grow(device, queue);
        self.upload_allocation(data, &allocation, queue);
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

        // Split a linear allocation at row boundaries to keep write_texture
        // rectangles contiguous without changing the shader's linear addressing.
        let first_line = (SIZE - x).min(size);
        if first_line != 0 {
            blocks.push([x, y, first_line, first_line, 1]);
        }
        let remaining = size - first_line;
        let full_lines = remaining / SIZE;
        let last_line = remaining % SIZE;
        if full_lines != 0 {
            blocks.push([0, y + 1, full_lines * SIZE, SIZE, full_lines]);
        }
        if last_line != 0 {
            blocks.push([0, y + 1 + full_lines, last_line, last_line, 1]);
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

            queue.write_texture(wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: *x,
                    y: *y,
                    z: layer as u32,
                },
                aspect: wgpu::TextureAspect::default()
            }, data_slice, wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(*height),
            }, extent);

            offset += byte_size;
        });
    }

    fn grow(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let old_layers = self.texture.depth_or_array_layers();
        if self.layer_count() as u32 <= old_layers {
            return;
        }

        let new_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Atlas Texture"),
            size: wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: self.layer_count() as u32,
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

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Atlas growth copies"),
        });
        for i in 0..old_layers {
            let layer = if i % 2 == 0 {
                self.layers.get(i as usize / 2)
            } else {
                self.band_layers.get(i as usize / 2)
            };
            if layer.is_none_or(Layer::is_empty) {
                continue;
            }
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: i as u32,
                    },
                    aspect: wgpu::TextureAspect::default()
                },
                wgpu::TexelCopyTextureInfo {
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

        // Queue writes are flushed before this submission; later writes then
        // follow the copy, so they cannot be overwritten by atlas growth.
        queue.submit(Some(encoder.finish()));
        self.texture = new_texture;
        self.texture_view = self.texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
    }
}
// Keep layer selection independent of GPU resources; texture growth follows allocation.
fn allocate_layer(layers: &mut Vec<Layer>, width: u32) -> Option<Allocation> {
    for (i, layer) in layers.iter_mut().enumerate() {
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
        layers.push(Layer::Busy(allocator));

        return Some(Allocation {
            region,
            layer: layers.len() - 1,
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocation_fills_layer_then_starts_next() {
        let mut layers = vec![Layer::Empty];
        let first = allocate_layer(&mut layers, SIZE * SIZE - 8).unwrap();
        let tail = allocate_layer(&mut layers, 8).unwrap();
        let next = allocate_layer(&mut layers, 8).unwrap();
        assert_eq!(
            [
                (first.layer(), first.position(), first.size()),
                (tail.layer(), tail.position(), tail.size()),
                (next.layer(), next.position(), next.size())
            ],
            [
                (0, [0, 0], SIZE * SIZE - 8),
                (0, [SIZE - 8, SIZE - 1], 8),
                (1, [0, 0], 8)
            ]
        );
        assert_eq!(layers.len(), 2);
        assert!(layers.iter().all(|layer| !layer.is_empty()));
    }

    #[test]
    fn oversized_allocation_leaves_empty_layer() {
        let mut layers = vec![Layer::Empty];
        assert!(allocate_layer(&mut layers, SIZE * SIZE + 1).is_none());
        assert!(layers.len() == 1 && layers[0].is_empty());
    }
}
