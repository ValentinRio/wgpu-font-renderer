use super::allocator::Region;

#[derive(Debug)]
/// A contiguous row-major run of floats in one atlas layer.
pub struct Allocation {
    /// Zero-based texture array layer.
    pub layer: usize,
    /// Float-slot start and count; a region may span rows.
    pub region: Region,
}

impl Allocation {
    /// Start float-slot [x, y], origin top left, y downward.
    pub fn position(&self) -> [u32; 2] {
        self.region.position()
    }

    /// Number of floats, including segment padding.
    pub fn size(&self) -> u32 {
        self.region.size()
    }

    /// Zero-based texture array layer index.
    pub fn layer(&self) -> usize {
        self.layer
    }
}