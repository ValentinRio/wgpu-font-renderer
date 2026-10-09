#[derive(Debug)]
/// Append-only row-major allocator for a square float-slot layer.
pub struct Allocator {
    offset: u32,
    size: u32,
    allocations: usize,
}

#[derive(Debug)]
/// Contiguous linear run of float slots, potentially spanning several rows.
pub struct Region {
    position: [u32; 2],
    size: u32,
}

impl Region {
    /// Start float-slot [x, y], origin top left, x rightward, y downward.
    pub fn position(&self) -> [u32; 2] {
        self.position
    }

    /// Number of float slots in the run.
    pub fn size(&self) -> u32 {
        self.size
    }
}

impl Allocator {
    /// Create a layer with `size` float slots per side. Use a nonzero side whose
    /// square fits u32 and is exactly representable as f32 (as for the atlas).
    pub fn new(size: u32) -> Allocator {
        Allocator {
            offset: 0,
            size,
            allocations: 0,
        }
    }

    /// Reserve a contiguous run of `size` float slots, or return `None` if full.
    /// Failed requests leave the allocator unchanged.
    /// Panics on layer-area overflow in debug builds.
    pub fn allocate(&mut self, size: u32) -> Option<Region> {
        // Flatten rows so outline streams need no per-glyph rectangular padding.
        let x =self.offset as f32 % self.size as f32;
        let row_index = f32::floor(self.offset as f32 / self.size as f32);
        let total_size = (self.size * self.size) as f32;
        let size_left = total_size - self.offset as f32;
        if size as f32 > size_left {
            None
        } else {
            self.offset += size;
            self.allocations += 1;
            Some(Region {
                position: [x as u32, row_index as u32],
                size,
            })
        }
    }

    // pub fn is_empty(&self) -> bool {
    //     self.allocations == 0
    // }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocations_wrap_rows_and_exhaust_exact_capacity() {
        let mut allocator = Allocator::new(4);
        let regions: Vec<_> = [3, 3, 10]
            .into_iter()
            .map(|size| {
                let region = allocator.allocate(size).unwrap();
                (region.position(), region.size())
            })
            .collect();
        assert_eq!(regions, [([0, 0], 3), ([3, 0], 3), ([2, 1], 10)]);
        assert!(allocator.allocate(1).is_none());
    }

    #[test]
    fn failed_allocation_preserves_remaining_capacity() {
        let mut allocator = Allocator::new(4);
        allocator.allocate(8).unwrap();
        assert!(allocator.allocate(9).is_none());
        assert_eq!(allocator.allocate(8).unwrap().position(), [0, 2]);
    }
}
