pub struct RunQueue {
    items: Vec<u32>,
    head: usize,
    limit: usize,
}

impl RunQueue {
    pub fn new(limit: usize) -> RunQueue {
        RunQueue {
            items: Vec::new(),
            head: 0,
            limit,
        }
    }

    pub fn full(&self) -> bool {
        self.items.len() - self.head >= self.limit
    }

    pub fn parts(&self) -> (&Vec<u32>, usize) {
        (&self.items, self.head)
    }

    pub fn parts_mut(&mut self) -> (&mut Vec<u32>, &mut usize) {
        (&mut self.items, &mut self.head)
    }
}
