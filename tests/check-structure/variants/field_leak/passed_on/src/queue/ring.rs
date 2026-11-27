pub struct RunQueue {
    slots: Slots,
    limit: usize,
}

impl RunQueue {
    pub fn new(limit: usize) -> RunQueue {
        RunQueue {
            slots: Slots::new(),
            limit,
        }
    }

    pub fn full(&self) -> bool {
        self.slots.waiting() >= self.limit
    }

    pub fn push(&mut self, task: u32) {
        self.slots.push(task)
    }

    pub fn pop(&mut self) -> Option<u32> {
        self.slots.pop()
    }

    pub fn waiting(&self) -> usize {
        self.slots.waiting()
    }
}

struct Slots {
    items: Vec<u32>,
    head: usize,
}

impl Slots {
    fn new() -> Slots {
        Slots {
            items: Vec::new(),
            head: 0,
        }
    }

    fn push(&mut self, task: u32) {
        self.items.push(task);
    }

    fn pop(&mut self) -> Option<u32> {
        let task = self.items.get(self.head).copied()?;
        self.head += 1;
        Some(task)
    }

    fn waiting(&self) -> usize {
        self.items.len() - self.head
    }
}
