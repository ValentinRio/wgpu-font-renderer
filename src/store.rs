use swash::CacheKey;
use skrifa::MetadataProvider;
use wgpu::CommandEncoderDescriptor;

use std::collections::HashMap;

use crate::{atlas::Atlas, loader::Font, LoadingError};

/// Opaque handle to a font loaded into a particular store.
/// IDs from different stores are never equal. With an ID from another store,
/// [`crate::TextRenderer::prepare`] skips the paragraph and
/// [`crate::TypeWriter::shape_text`] returns `None`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct FontId(CacheKey);

/// Line metrics in physical pixels at the requested font size.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontMetrics {
    /// Positive baseline distance from the paragraph's top anchor.
    pub ascent: f32,
    /// Positive distance below the baseline.
    pub descent: f32,
    /// Additional spacing between lines.
    pub line_gap: f32,
}

/// A variable font axis, with values in its design space.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Axis {
    /// Four-byte OpenType axis tag.
    pub tag: [u8; 4],
    /// Minimum axis value.
    pub min: f32,
    /// Default axis value.
    pub default: f32,
    /// Maximum axis value.
    pub max: f32,
}

/// Owns loaded font data and the shared GPU outline atlas.
/// Later loads are picked up by the renderer on its next preparation.
pub struct FontStore {
    cache: HashMap<FontId, Font>,
    atlas: Atlas,
}

impl FontStore {
    /// Create an empty store and Rgba32Float atlas on `device`.
    /// wgpu validation errors occur if the device cannot support the atlas.
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            cache: HashMap::new(),
            atlas: Atlas::new(device),
        }
    }

    /// Read face zero from a font file and cache outlines for `chars`.
    ///
    /// Returns [`LoadingError::Io`] for any file read failure, or
    /// [`LoadingError::InvalidFont`] for invalid font data. On wasm, use
    /// [`Self::load_from_bytes`] instead of filesystem access.
    /// Device and queue must belong together; GPU failures follow wgpu validation.
    pub fn load(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        path: impl AsRef<std::path::Path>,
        chars: &str
    ) -> Result<FontId, LoadingError>{

        let data = std::fs::read(path).map_err(LoadingError::Io)?;
        self.load_from_bytes(device, queue, &data, chars)
    }

    /// Load face zero from font bytes and upload outlines for `chars`.
    ///
    /// Returns a fresh font key, or [`LoadingError::InvalidFont`] for invalid data.
    /// Glyph zero is always cached if it has an outline, so missing characters
    /// draw `.notdef`. Glyphs without outlines still advance without drawing.
    /// Returns [`LoadingError::AtlasFull`] if an outline or its bands cannot fit
    /// one layer or allocation exceeds the device texture array layer limit.
    /// Each default layer uses 16 MiB (initially 32 MiB; 4 GiB at 256 layers).
    /// Growth temporarily retains both old and new textures. Previous
    /// fonts remain usable and the failed font is not added.
    /// Device and queue must belong together; GPU failures follow wgpu validation.
    /// Outline coordinates in the atlas are font units, not screen pixels.
    pub fn load_from_bytes(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        data: &[u8],
        chars: &str
    ) -> Result<FontId, LoadingError> {
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor { label: None });

        let font = Font::from_instance(device, &mut encoder, queue, data.into(), chars.into(), &[], &mut self.atlas)?;

        queue.submit(Some(encoder.finish()));

        let cache_key = FontId(font.key);

        self.cache.insert(cache_key, font);

        Ok(cache_key)
    }

    /// Return line metrics at `size` physical pixels per em.
    /// The baseline is at paragraph y plus `ascent`. Returns `None` for a
    /// foreign ID or a nonfinite or nonpositive size.
    pub fn metrics(&self, font: FontId, size: f32) -> Option<FontMetrics> {
        if !size.is_finite() || size <= 0. { return None; }
        let font = self.get(font)?;
        let scale = size / f32::from(font.face().metrics(skrifa::instance::Size::unscaled(),
            &font.location).units_per_em);
        Some(FontMetrics { ascent: font.metrics.ascent * scale,
            descent: font.metrics.descent * scale, line_gap: font.metrics.line_gap * scale })
    }

    /// Return design-space axes, or `None` for a foreign ID.
    /// Non-variable fonts return an empty vector.
    pub fn axes(&self, font: FontId) -> Option<Vec<Axis>> {
        Some(self.get(font)?.face().axes().iter().map(|axis| Axis {
            tag: axis.tag().to_be_bytes(), min: axis.min_value(),
            default: axis.default_value(), max: axis.max_value(),
        }).collect())
    }

    /// Load a fresh instance sharing the base's bytes and cached character set.
    /// Values clamp to axis ranges; unknown tags are ignored, omitted axes use
    /// their defaults, and duplicate tags use the last value. Non-variable
    /// fonts produce identical instances. Returns [`LoadingError::UnknownFont`]
    /// for a foreign base. Other loading errors and GPU requirements are the
    /// same as [`Self::load_from_bytes`].
    pub fn load_variation(&mut self, device: &wgpu::Device, queue: &wgpu::Queue,
        base: FontId, axes: &[([u8; 4], f32)]) -> Result<FontId, LoadingError> {
        let base = self.get(base).ok_or(LoadingError::UnknownFont)?;
        let data = base.data.clone();
        let characters = base.characters.clone();
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor { label: None });
        let font = Font::from_instance(device, &mut encoder, queue, data, characters, axes, &mut self.atlas)?;
        queue.submit(Some(encoder.finish()));
        let id = FontId(font.key);
        self.cache.insert(id, font);
        Ok(id)
    }

    #[cfg(feature = "unstable-internals")]
    pub(crate) fn pad_atlas(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, floats: [u32; 2]) {
        let mut encoder = device.create_command_encoder(&Default::default());
        if floats[0] > 0 {
            self.atlas.upload(floats[0], &vec![0; floats[0] as usize * 4], device, &mut encoder, queue).expect("curve padding fits");
        }
        if floats[1] > 0 {
            self.atlas.upload_bands(floats[1], &vec![0; floats[1] as usize * 4], device, &mut encoder, queue).expect("band padding fits");
        }
        queue.submit(Some(encoder.finish()));
    }

    /// Borrow the shared GPU outline atlas for renderer construction.
    pub(crate) fn atlas(&self) -> &Atlas {
        &self.atlas
    }

    /// Look up a loaded font by key; returns `None` for an unknown key.
    pub(crate) fn get(&self, font_key: FontId) -> Option<&Font> {
        self.cache.get(&font_key)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "../tests/unit/store_behaviour.rs"]
mod behaviour_tests;
