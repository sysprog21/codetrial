pub struct RunQueue {
    items: Vec<u32>,
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

    pub fn push(&mut self, task: u32) -> bool {
        if self.backlog() >= self.limit {
            return false;
        }
        self.items.push(task);
        true
    }

    pub fn pop(&mut self) -> Option<u32> {
        let task = self.items.get(self.head).copied()?;
        self.head += 1;
        Some(task)
    }

    pub fn backlog(&self) -> usize {
        self.items.len() - self.head
    }

    pub fn limit(&self) -> usize {
        self.limit
    }
}
