use super::timers::Timers;

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

impl Timers {
    pub fn new() -> Timers {
        Timers { due: Vec::new() }
    }

    pub fn arm(&mut self, id: u32, at: u64) {
        self.due.push((id, at));
    }

    pub fn fire(&mut self, now: u64, tasks: &mut Tasks) {
        for &(id, at) in &self.due {
            if at <= now {
                tasks.wake(id);
            }
        }
        self.due.retain(|&(_, at)| at > now);
    }
}
