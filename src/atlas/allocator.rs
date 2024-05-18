#[derive(Debug)]
pub struct Allocator {
    offset: u32,
    size: u32,
    allocations: usize,
}

#[derive(Debug)]
pub struct Region {
    position: [u32; 2],
    size: u32,
}

impl Region {
    pub fn position(&self) -> [u32; 2] {
        self.position
    }

    pub fn size(&self) -> u32 {
        self.size
    }
}

impl Allocator {
    pub fn new(size: u32) -> Allocator {
        Allocator {
            offset: 0,
            size,
            allocations: 0,
        }
    }

    pub fn allocate(&mut self, size: u32) -> Option<Region> {
        let x =self.offset as f32 % self.size as f32;
        let row_index = f32::floor(self.offset as f32 / self.size as f32);
        let total_size = (self.size * self.size) as f32;
        let size_left = total_size - self.offset as f32;
        self.offset += size;
        if size as f32 > size_left {
            None
        } else {
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