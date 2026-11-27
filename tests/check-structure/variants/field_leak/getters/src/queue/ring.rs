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

    pub fn items(&self) -> &Vec<u32> {
        &self.items
    }

    pub fn items_mut(&mut self) -> &mut Vec<u32> {
        &mut self.items
    }

    pub fn head(&self) -> usize {
        self.head
    }

    pub fn head_mut(&mut self) -> &mut usize {
        &mut self.head
    }
}
