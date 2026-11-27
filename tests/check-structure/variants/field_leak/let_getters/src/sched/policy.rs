use crate::queue::RunQueue;

pub fn admit(queue: &mut RunQueue, task: u32) -> bool {
    if queue.full() {
        return false;
    }
    queue.with_items(|items| items.push(task));
    true
}

pub fn next(queue: &mut RunQueue) -> Option<u32> {
    let task = queue.items().get(queue.head()).copied()?;
    queue.set_head(queue.head() + 1);
    Some(task)
}
