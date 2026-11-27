use crate::queue::RunQueue;

pub fn admit(queue: &mut RunQueue, task: u32) -> bool {
    if queue.full() {
        return false;
    }
    let (items, _) = queue.parts_mut();
    items.push(task);
    true
}

pub fn next(queue: &mut RunQueue) -> Option<u32> {
    let (items, head) = queue.parts_mut();
    let task = items.get(*head).copied()?;
    *head += 1;
    Some(task)
}
