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

pub fn admit(queue: &mut RunQueue, task: u32) -> bool {
    if queue.full() {
        return false;
    }
    queue.items.push(task);
    true
}

pub fn next(queue: &mut RunQueue) -> Option<u32> {
    let task = queue.items.get(queue.head).copied()?;
    queue.head += 1;
    Some(task)
}

pub fn backlog(queue: &RunQueue) -> usize {
    queue.items.len() - queue.head
}

pub fn drained(queue: &RunQueue) -> bool {
    queue.head == queue.items.len()
}
