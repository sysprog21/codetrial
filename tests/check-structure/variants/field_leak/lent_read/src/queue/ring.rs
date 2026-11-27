/// The most tasks a queue holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limit(pub usize);

pub struct RunQueue {
    items: Vec<u32>,
    pub(crate) head: usize,
    limit: Limit,
}

impl RunQueue {
    pub fn new(limit: usize) -> RunQueue {
        RunQueue {
            items: Vec::new(),
            head: 0,
            limit: Limit(limit),
        }
    }

    pub fn push(&mut self, task: u32) -> bool {
        if self.backlog() >= self.limit.0 {
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

    pub fn limit(&self) -> &Limit {
        &self.limit
    }
}
