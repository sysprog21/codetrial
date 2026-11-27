pub mod tasks;
pub mod timers;

use tasks::Tasks;
use timers::Timers;

pub fn tick(now: u64, tasks: &mut Tasks, timers: &mut Timers) {
    for id in timers.fire(now) {
        tasks.wake(id);
    }
}
