pub mod timers;

use timers::Timers;

pub struct Task {
    pub id: u32,
    pub asleep: bool,
}

pub struct Tasks {
    list: Vec<Task>,
}

impl Tasks {
    pub fn new() -> Tasks {
        Tasks { list: Vec::new() }
    }

    pub fn spawn(&mut self, id: u32, timers: &mut Timers) {
        self.list.push(Task { id, asleep: true });
        timers.arm(id, 10);
    }

    pub fn wake(&mut self, id: u32) {
        for task in &mut self.list {
            if task.id == id {
                task.asleep = false;
            }
        }
    }

    pub fn runnable(&self) -> usize {
        self.list.iter().filter(|task| !task.asleep).count()
    }
}

pub fn tick(now: u64, tasks: &mut Tasks, timers: &mut Timers) {
    for id in timers.fire(now) {
        tasks.wake(id);
    }
}
