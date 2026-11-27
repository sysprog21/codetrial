use crate::queue::RunQueue;

pub fn backlog(queue: &RunQueue) -> usize {
    let (items, head) = queue.parts();
    items.len() - head
}

pub fn drained(queue: &RunQueue) -> bool {
    let (items, head) = queue.parts();
    head == items.len()
}
