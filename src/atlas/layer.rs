use super::allocator::Allocator;

#[derive(Debug)]
/// Logical allocation state of an atlas layer.
pub enum Layer {
    /// No regions have been allocated.
    Empty,
    /// Append-only allocator with at least one successful request.
    Busy(Allocator),
}

impl Layer {
    /// Whether this layer has no allocator yet.
    pub fn is_empty(&self) -> bool {
        matches!(self, Layer::Empty)
    }
}