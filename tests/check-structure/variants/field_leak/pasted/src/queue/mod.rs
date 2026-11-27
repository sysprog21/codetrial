mod ring;
pub use ring::{RunQueue, admit, backlog, drained, next};

pub fn fresh(limit: usize) -> RunQueue {
    RunQueue::new(limit)
}
