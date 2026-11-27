pub struct RunQueue {
    pub(crate) items: Vec<u32>,
    pub(crate) head: usize,
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
}
