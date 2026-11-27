use crate::kernel::tasks::Tasks;
use crate::kernel::timers::Timers;

pub fn banner(width: usize) -> String {
    "-".repeat(width)
}

pub fn prompt(name: &str) -> String {
    format!("{name}> ")
}

pub fn boot(ids: &[u32], until: u64) -> usize {
    let mut tasks = Tasks::new();
    let mut timers = Timers::new();
    for &id in ids {
        tasks.spawn(id, &mut timers);
    }
    let mut awake = 0;
    for now in 0..until {
        timers.fire(now, &mut tasks);
        awake = tasks.runnable();
    }
    awake
}
