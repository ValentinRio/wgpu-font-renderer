use super::allocator::Region;

#[derive(Debug)]
pub struct Allocation {
    pub layer: usize,
    pub region: Region,
}

impl Allocation {
    pub fn position(&self) -> [u32; 2] {
        self.region.position()
    }

    pub fn size(&self) -> u32 {
        self.region.size()
    }

    pub fn layer(&self) -> usize {
        self.layer
    }
}