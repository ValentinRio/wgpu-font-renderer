use super::allocator::Allocator;

#[derive(Debug)]
pub enum Layer {
    Empty,
    Busy(Allocator),
}

impl Layer {
    pub fn is_empty(&self) -> bool {
        matches!(self, Layer::Empty)
    }
}