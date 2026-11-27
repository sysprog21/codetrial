use crate::queue::RunQueue;

pub fn backlog(queue: &RunQueue) -> usize {
    queue.backlog()
}

pub fn drained(queue: &RunQueue) -> bool {
    queue.backlog() == 0
}
