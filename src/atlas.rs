/// Atlas region and layer metadata.
pub mod allocation;
/// Linear allocation within a logical atlas layer.
pub mod allocator;
/// Empty/busy layer bookkeeping.
pub mod layer;

use self::{allocation::Allocation, allocator::Allocator, layer::Layer};
use std::sync::atomic::{AtomicUsize, Ordering};
static NEXT_VERSION: AtomicUsize = AtomicUsize::new(1);

/// Float slots per logical row and rows per logical layer.
pub const SIZE: u32 = 2048;

/// Packed append-only outline and band storage. Logical allocators enforce the
/// per-stream limit; each allocation also records its permanent packed address.
/// CPU instance construction resolves that address, with no GPU offset table.
pub struct Atlas {
    texture: wgpu::Texture,
    texture_view: wgpu::TextureView,
    layers: Vec<Layer>,
    band_layers: Vec<Layer>,
    pending: Vec<(u64, Vec<u8>)>,
    used: u64,
    committed: u64,
    loading: bool,
    pub(crate) version: usize,
    pub(crate) max_layers: usize,
    pub(crate) max_rows: u32,
    pub(crate) max_physical_layers: u32,
}

type Checkpoint = (Vec<Layer>, Vec<Layer>, usize, u64);

/// A font load rolls back allocations and staged writes on error or unwinding.
pub(crate) struct Load<'a> {
    atlas: &'a mut Atlas,
    checkpoint: Option<Checkpoint>,
}
impl std::ops::Deref for Load<'_> {
    type Target = Atlas;
    fn deref(&self) -> &Atlas {
        self.atlas
    }
}
impl std::ops::DerefMut for Load<'_> {
    fn deref_mut(&mut self) -> &mut Atlas {
        self.atlas
    }
}
impl Load<'_> {
    pub(crate) fn finish(mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
        if !self.atlas.commit(device, queue) {
            return false;
        }
        self.checkpoint = None;
        true
    }
}
impl Drop for Load<'_> {
    fn drop(&mut self) {
        if let Some(checkpoint) = self.checkpoint.take() {
            self.atlas.restore(checkpoint);
        }
    }
}

// Pure so device limits and the u32 packed-float address cap are testable without
// enormous textures. Round-robin rows leave less than one 8 KiB row per layer.
fn texture_shape(floats: u64, max_rows: u32, max_layers: u32) -> Option<wgpu::Extent3d> {
    if floats > u64::from(u32::MAX) || max_rows == 0 || max_layers == 0 {
        return None;
    }
    let rows = floats.div_ceil(u64::from(SIZE)).max(1);
    let depth = rows.div_ceil(u64::from(max_rows));
    if depth > u64::from(max_layers) {
        return None;
    }
    Some(wgpu::Extent3d {
        width: SIZE / 4,
        height: rows.div_ceil(depth) as u32,
        depth_or_array_layers: depth as u32,
    })
}

impl Atlas {
    /// Create an empty atlas occupying one 512-texel row.
    pub fn new(device: &wgpu::Device) -> Self {
        let texture = Self::texture(device, texture_shape(0, 1, 1).unwrap());
        let texture_view = Self::view_of(&texture);
        Self {
            texture,
            texture_view,
            layers: vec![Layer::Empty],
            band_layers: vec![Layer::Empty],
            pending: Vec::new(),
            used: 0,
            committed: 0,
            loading: false,
            version: NEXT_VERSION.fetch_add(1, Ordering::Relaxed),
            max_layers: device.limits().max_texture_array_layers as usize,
            max_rows: SIZE.min(device.limits().max_texture_dimension_2d),
            max_physical_layers: device.limits().max_texture_array_layers,
        }
    }
    fn texture(device: &wgpu::Device, size: wgpu::Extent3d) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Atlas Texture"),
            size,
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
    /// Borrow the current array view; only growth replaces it.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.texture_view
    }
    #[cfg(any(test, feature = "unstable-internals"))]
    pub fn resources(&self) -> &wgpu::TextureView {
        self.view()
    }
    /// Number of logical allocation layers, including outline and band layers.
    #[cfg(any(test, feature = "unstable-internals"))]
    pub fn layer_count(&self) -> usize {
        (self.layers.len() * 2 - 1).max(self.band_layers.len() * 2)
    }
    #[cfg(feature = "unstable-internals")]
    pub(crate) fn written_bytes(&self) -> u64 {
        self.used * 4
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
    fn checkpoint(&self) -> Checkpoint {
        (
            self.layers.clone(),
            self.band_layers.clone(),
            self.pending.len(),
            self.used,
        )
    }
    pub(crate) fn begin_load(&mut self) -> Load<'_> {
        assert!(!self.loading, "nested atlas load");
        let checkpoint = self.checkpoint();
        self.loading = true;
        Load {
            atlas: self,
            checkpoint: Some(checkpoint),
        }
    }
    fn restore(&mut self, checkpoint: Checkpoint) {
        (self.layers, self.band_layers) = (checkpoint.0, checkpoint.1);
        self.pending.truncate(checkpoint.2);
        self.used = checkpoint.3;
        self.loading = false;
    }
    /// Reserve whole RGBA texels in a logical outline layer.
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
    /// Reserve whole RGBA texels in a logical band layer.
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
        let bytes = data[..size as usize * 4].to_vec();
        let checkpoint = (!self.loading).then(|| self.checkpoint());
        let (layers, limit) = if band {
            (&mut self.band_layers, self.max_layers / 2)
        } else {
            (&mut self.layers, self.max_layers.div_ceil(2))
        };
        let mut a = allocate_layer(layers, size, limit)?;
        a.layer = a.layer * 2 + usize::from(band);
        a.packed = self.used;
        self.pending.push((self.used, bytes));
        self.used += u64::from(size);
        if !self.loading && !self.commit(device, queue) {
            self.restore(checkpoint.unwrap());
            return None;
        }
        Some(a)
    }
    fn origin(offset: u64, depth: u32) -> wgpu::Origin3d {
        let row = offset / u64::from(SIZE);
        wgpu::Origin3d {
            x: (offset % u64::from(SIZE)) as u32 / 4,
            y: (row / u64::from(depth)) as u32,
            z: (row % u64::from(depth)) as u32,
        }
    }
    fn commit(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
        let Some(shape) = texture_shape(self.used, self.max_rows, self.max_physical_layers) else {
            return false;
        };
        let grown = if self.texture.size() != shape {
            let texture = Self::texture(device, shape);
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Atlas growth copies"),
            });
            let mut src = 0;
            while src < self.committed {
                let n = (self.committed - src).min(u64::from(SIZE) - src % u64::from(SIZE));
                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo {
                        origin: Self::origin(src, self.texture.depth_or_array_layers()),
                        ..self.texture.as_image_copy()
                    },
                    wgpu::TexelCopyTextureInfo {
                        origin: Self::origin(src, shape.depth_or_array_layers),
                        ..texture.as_image_copy()
                    },
                    wgpu::Extent3d {
                        width: n as u32 / 4,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                );
                src += n;
            }
            queue.submit(Some(encoder.finish()));
            Some(texture)
        } else {
            None
        };
        let target = grown.as_ref().unwrap_or(&self.texture);
        let depth = target.depth_or_array_layers();
        for (mut dst, bytes) in self.pending.drain(..) {
            let mut data = bytes.as_slice();
            while !data.is_empty() {
                let n =
                    (data.len() / 4).min((u64::from(SIZE) - dst % u64::from(SIZE)) as usize) as u32;
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        origin: Self::origin(dst, depth),
                        ..target.as_image_copy()
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
                dst += u64::from(n);
            }
        }
        if let Some(texture) = grown {
            let view = Self::view_of(&texture);
            self.texture_view = view;
            self.texture = texture;
            self.version = NEXT_VERSION.fetch_add(1, Ordering::Relaxed);
        }
        self.committed = self.used;
        self.loading = false;
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
                        packed: 0,
                    });
                }
            }
            Layer::Busy(allocator) => {
                if let Some(region) = allocator.allocate(width) {
                    return Some(Allocation {
                        region,
                        layer: i,
                        packed: 0,
                    });
                }
            }
        }
    }

    if layers.len() >= max_layers {
        return None;
    }
    let mut allocator = Allocator::new(SIZE);

    if let Some(region) = allocator.allocate(width) {
        layers.push(Layer::Busy(allocator));

        return Some(Allocation {
            region,
            layer: layers.len() - 1,
            packed: 0,
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

#[cfg(all(test, not(target_arch = "wasm32")))]
mod review_tests {
    use super::*;
    #[test]
    fn packed_shape_rejects_depth_and_address_exhaustion() {
        let shape = texture_shape(u64::from(SIZE) * 3, 2, 2).unwrap();
        assert_eq!((shape.height, shape.depth_or_array_layers), (2, 2));
        assert!(texture_shape(u64::from(SIZE) * 4, 2, 2).is_some());
        assert!(texture_shape(u64::from(SIZE) * 4 + 4, 2, 2).is_none());
        assert!(texture_shape(u64::from(u32::MAX), 2048, 2048).is_some());
        assert!(texture_shape(u64::from(u32::MAX) + 1, 2048, 2048).is_none());
        assert!(texture_shape(0, 0, 1).is_none());
        assert!(texture_shape(0, 1, 0).is_none());
    }
    #[test]
    fn unwinding_load_rolls_back_and_append_reuses_texture() {
        let Some((device, queue, _)) = crate::atlas_tests::common::gpu() else {
            return;
        };
        let mut atlas = Atlas::new(&device);
        let mut encoder = device.create_command_encoder(&Default::default());
        atlas
            .upload(
                4,
                bytemuck::cast_slice(&[1_f32, 2., 3., 4.]),
                &device,
                &mut encoder,
                &queue,
            )
            .unwrap();
        let snapshot = (
            atlas.texture.clone(),
            atlas.texture.size(),
            atlas.layer_count(),
            atlas.version,
            atlas.used_floats(),
        );
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut load = atlas.begin_load();
            load.upload_bands(4, &[0; 16], &device, &mut encoder, &queue)
                .unwrap();
            panic!("injected loader unwind");
        }));
        assert!(caught.is_err());
        assert!(!atlas.loading);
        assert!(atlas.pending.is_empty());
        assert_eq!(
            (
                atlas.texture.clone(),
                atlas.texture.size(),
                atlas.layer_count(),
                atlas.version,
                atlas.used_floats()
            ),
            snapshot
        );
        let mut load = atlas.begin_load();
        load.upload_bands(4, &[0; 16], &device, &mut encoder, &queue)
            .unwrap();
        assert!(load.finish(&device, &queue));
        assert_eq!(
            atlas.texture, snapshot.0,
            "spare capacity must be used without copying/rebuilding"
        );
        assert_eq!(atlas.version, snapshot.3);
        assert_eq!(atlas.used, 8);
    }
}
