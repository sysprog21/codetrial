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
        let items = &self.items;
        items
    }

    pub fn with_items<R>(&mut self, each: impl FnOnce(&mut Vec<u32>) -> R) -> R {
        each(&mut self.items)
    }

    pub fn head(&self) -> usize {
        let head = self.head;
        head
    }

    pub fn set_head(&mut self, head: usize) {
        self.head = head;
    }
}
