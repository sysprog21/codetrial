use crate::queue::RunQueue;

pub fn admit(queue: &mut RunQueue, task: u32) -> bool {
    if queue.full() {
        return false;
    }
    queue.push(task);
    true
}

pub fn next(queue: &mut RunQueue) -> Option<u32> {
    queue.pop()
}
