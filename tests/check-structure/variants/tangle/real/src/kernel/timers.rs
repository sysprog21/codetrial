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

    pub fn fire(&mut self, now: u64) -> Vec<u32> {
        let fired = self.due.iter().filter(|&&(_, at)| at <= now).map(|&(id, _)| id).collect();
        self.due.retain(|&(_, at)| at > now);
        fired
    }
}
