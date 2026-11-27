use crate::queue::RunQueue;

pub fn backlog(queue: &RunQueue) -> usize {
    queue.waiting()
}

pub fn drained(queue: &RunQueue) -> bool {
    queue.waiting() == 0
}
