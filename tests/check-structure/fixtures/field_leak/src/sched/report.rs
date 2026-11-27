use crate::queue::RunQueue;

pub fn backlog(queue: &RunQueue) -> usize {
    queue.items.len() - queue.head
}

pub fn drained(queue: &RunQueue) -> bool {
    queue.head == queue.items.len()
}
