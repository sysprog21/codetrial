mod ring;
pub use ring::RunQueue;

pub fn fresh(limit: usize) -> RunQueue {
    RunQueue::new(limit)
}
