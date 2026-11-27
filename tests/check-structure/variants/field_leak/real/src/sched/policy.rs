use crate::queue::RunQueue;

pub fn admit(queue: &mut RunQueue, task: u32) -> bool {
    queue.push(task)
}

pub fn next(queue: &mut RunQueue) -> Option<u32> {
    queue.pop()
}
