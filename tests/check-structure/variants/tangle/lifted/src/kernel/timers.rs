use super::Tasks;

pub struct Timers {
    due: Vec<(u32, u64)>,
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
