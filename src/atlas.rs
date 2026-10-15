/// Atlas region and layer metadata.
pub mod allocation;
/// Linear allocation within a logical atlas layer.
pub mod allocator;
/// Empty/busy layer bookkeeping.
pub mod layer;

use self::{allocation::Allocation, allocator::Allocator, layer::Layer};
use std::sync::atomic::{AtomicUsize, Ordering};
use wgpu::util::DeviceExt;
static NEXT_VERSION: AtomicUsize = AtomicUsize::new(1);

/// Float slots per logical row and rows per logical layer.
/// Logical addresses stay stable; physical texture storage packs only used prefixes.
pub const SIZE: u32 = 2048;

/// Packed Rgba32Float outline and band storage, with stable logical allocations.
pub struct Atlas {
    texture: wgpu::Texture,
    texture_view: wgpu::TextureView,
    offsets_buffer: wgpu::Buffer,
    offsets: Vec<u32>,
    committed: Vec<u32>,
    layers: Vec<Layer>,
    band_layers: Vec<Layer>,
    pending: Vec<(usize, u32, Vec<u8>)>,
    loading: bool,
    pub(crate) version: usize,
    pub(crate) max_layers: usize,
}

impl Atlas {
    /// Create an empty atlas: one 512-texel row, plus a small address buffer.
    pub fn new(device: &wgpu::Device) -> Self {
        let texture = Self::texture(device, 1, 1);
        let texture_view = Self::view_of(&texture);
        let offsets_buffer = Self::offset_buffer(device, &[0, 0]);
        Self {
            texture,
            texture_view,
            offsets_buffer,
            offsets: vec![0, 0],
            committed: vec![0, 0],
            layers: vec![Layer::Empty],
            band_layers: vec![Layer::Empty],
            pending: Vec::new(),
            loading: false,
            version: NEXT_VERSION.fetch_add(1, Ordering::Relaxed),
            max_layers: device.limits().max_texture_array_layers as usize,
        }
    }
    fn texture(device: &wgpu::Device, height: u32, depth: u32) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Atlas Texture"),
            size: wgpu::Extent3d {
                width: SIZE / 4,
                height,
                depth_or_array_layers: depth,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::COPY_DST
                 | wgpu::TextureUsages::COPY_SRC
                 | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    }
    fn view_of(texture: &wgpu::Texture) -> wgpu::TextureView {
        texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        })
    }
    fn offset_buffer(device: &wgpu::Device, offsets: &[u32]) -> wgpu::Buffer {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Atlas layer offsets"),
            usage: wgpu::BufferUsages::STORAGE,
            contents: bytemuck::cast_slice(offsets),
        })
    }
    /// Borrow the current array view; a successful load replaces it.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.texture_view
    }
    /// Packed logical-layer bases, in float slots.
    pub fn offsets_buffer(&self) -> &wgpu::Buffer {
        &self.offsets_buffer
    }
    /// Resources needed by an analytic shader bind group.
    #[cfg(any(test, feature = "unstable-internals"))]
    pub fn resources(&self) -> (&wgpu::TextureView, &wgpu::Buffer) {
        (self.view(), self.offsets_buffer())
    }
    /// Number of logical allocation layers, including outline and band layers.
    pub fn layer_count(&self) -> usize {
        (self.layers.len() * 2 - 1).max(self.band_layers.len() * 2)
    }
    fn logical_used(&self) -> Vec<u32> {
        (0..self.layer_count())
            .map(|i| {
                let layers = if i % 2 == 0 {
                    &self.layers
                } else {
                    &self.band_layers
                };
                match layers.get(i / 2) {
                    Some(Layer::Busy(a)) => a.used(),
                    _ => 0,
                }
            })
            .collect()
    }
    #[cfg(feature = "unstable-internals")]
    pub(crate) fn written_bytes(&self) -> u64 {
        self.logical_used()
            .into_iter()
            .map(|n| u64::from(n) * 4)
            .sum()
    }
    #[cfg(test)]
    pub(crate) fn used_floats(&self) -> [u32; 2] {
        [&self.layers, &self.band_layers].map(|layers| {
            layers
                .iter()
                .map(|layer| match layer {
                    Layer::Empty => 0,
                    Layer::Busy(a) => a.used(),
                })
                .sum()
        })
    }
    pub(crate) fn checkpoint(&self) -> (Vec<Layer>, Vec<Layer>, usize) {
        (
            self.layers.clone(),
            self.band_layers.clone(),
            self.pending.len(),
        )
    }
    pub(crate) fn begin_load(&mut self) {
        self.loading = true;
    }
    pub(crate) fn restore(&mut self, checkpoint: (Vec<Layer>, Vec<Layer>, usize)) {
        (self.layers, self.band_layers) = (checkpoint.0, checkpoint.1);
        self.pending.truncate(checkpoint.2);
        self.loading = false;
    }
    /// Reserve and upload whole RGBA texels into a logical outline layer.
    /// A stream must fit the logical 16 MiB layer and all storage must fit device limits.
    pub fn upload(
        &mut self,
        size: u32,
        data: &[u8],
        device: &wgpu::Device,
        _encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
    ) -> Option<Allocation> {
        self.reserve(size, data, device, queue, false)
    }
    /// Reserve and upload band data into logical odd layers.
    pub fn upload_bands(
        &mut self,
        size: u32,
        data: &[u8],
        device: &wgpu::Device,
        _encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
    ) -> Option<Allocation> {
        self.reserve(size, data, device, queue, true)
    }
    fn reserve(
        &mut self,
        size: u32,
        data: &[u8],
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        band: bool,
    ) -> Option<Allocation> {
        assert_eq!(size % 4, 0, "atlas uploads must contain whole RGBA texels");
        let checkpoint = (!self.loading).then(|| self.checkpoint());
        let (layers, limit) = if band {
            (&mut self.band_layers, self.max_layers / 2)
        } else {
            (&mut self.layers, self.max_layers.div_ceil(2))
        };
        let mut a = allocate_layer(layers, size, limit)?;
        a.layer = a.layer * 2 + usize::from(band);
        let [x, y] = a.position();
        self.pending
            .push((a.layer, y * SIZE + x, data[..size as usize * 4].to_vec()));
        if !self.loading && !self.commit(device, queue) {
            if let Some(checkpoint) = checkpoint {
                self.restore(checkpoint);
            }
            return None;
        }
        Some(a)
    }
    fn origin(offset: u32, depth: u32) -> wgpu::Origin3d {
        let row = offset / SIZE;
        wgpu::Origin3d {
            x: (offset % SIZE) / 4,
            y: row / depth,
            z: row % depth,
        }
    }
    pub(crate) fn commit(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
        self.loading = false;
        if self.pending.is_empty() {
            return true;
        }
        let used = self.logical_used();
        let total: u64 = used.iter().map(|&n| u64::from(n)).sum();
        if total > u64::from(u32::MAX) {
            return false;
        }
        let rows = (total as u32).div_ceil(SIZE).max(1);
        let depth = rows.div_ceil(SIZE.min(device.limits().max_texture_dimension_2d));
        if depth > device.limits().max_texture_array_layers {
            return false;
        }
        let height = rows.div_ceil(depth);
        // Pack used logical prefixes without holes. Round-robin physical rows keep
        // equal-height array layers within ONE 8 KiB row of their actual payload.
        // A tiny offset buffer preserves glyph addresses, so growth needs no font
        // repacking or shader-maths changes. Commit once per successful font load.
        let texture = Self::texture(device, height, depth);
        let mut offsets = Vec::with_capacity(used.len());
        let mut base = 0;
        for &count in &used {
            offsets.push(base);
            base += count;
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Atlas packing copies"),
        });
        for (i, &count) in self.committed.iter().enumerate() {
            let mut src = self.offsets[i];
            let mut dst = offsets[i];
            let mut remaining = count;
            while remaining > 0 {
                let n = remaining.min(SIZE - src % SIZE).min(SIZE - dst % SIZE);
                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo {
                        origin: Self::origin(src, self.texture.depth_or_array_layers()),
                        ..self.texture.as_image_copy()
                    },
                    wgpu::TexelCopyTextureInfo {
                        origin: Self::origin(dst, depth),
                        ..texture.as_image_copy()
                    },
                    wgpu::Extent3d {
                        width: n / 4,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                );
                src += n;
                dst += n;
                remaining -= n;
            }
        }
        // Flush earlier queue writes before copying; new uploads follow the copy.
        queue.submit(Some(encoder.finish()));
        for (layer, offset, bytes) in self.pending.drain(..) {
            let mut dst = offsets[layer] + offset;
            let mut data = bytes.as_slice();
            while !data.is_empty() {
                let n = (data.len() / 4).min((SIZE - dst % SIZE) as usize) as u32;
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        origin: Self::origin(dst, depth),
                        ..texture.as_image_copy()
                    },
                    &data[..n as usize * 4],
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(n * 4),
                        rows_per_image: Some(1),
                    },
                    wgpu::Extent3d {
                        width: n / 4,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                );
                data = &data[n as usize * 4..];
                dst += n;
            }
        }
        self.offsets_buffer = Self::offset_buffer(device, &offsets);
        self.offsets = offsets;
        self.committed = used;
        self.texture_view = Self::view_of(&texture);
        self.texture = texture;
        self.version = NEXT_VERSION.fetch_add(1, Ordering::Relaxed);
        true
    }
}
// Keep layer selection independent of GPU resources; texture growth follows allocation.
fn allocate_layer(layers: &mut Vec<Layer>, width: u32, max_layers: usize) -> Option<Allocation> {
    if max_layers == 0 {
        return None;
    }
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

    if layers.len() >= max_layers { return None; }
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
        let first = allocate_layer(&mut layers, SIZE * SIZE - 8, 2).unwrap();
        let tail = allocate_layer(&mut layers, 8, 2).unwrap();
        let next = allocate_layer(&mut layers, 8, 2).unwrap();
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
        assert!(allocate_layer(&mut layers, SIZE * SIZE + 1, 2).is_none());
        assert!(layers.len() == 1 && layers[0].is_empty());
    }
}
