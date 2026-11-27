use crate::queue::RunQueue;

pub fn backlog(queue: &RunQueue) -> usize {
    queue.backlog().min(queue.limit().0)
}

pub fn drained(queue: &RunQueue) -> bool {
    let limit = *queue.limit();
    queue.backlog() == 0 || queue.head == limit.0
}
