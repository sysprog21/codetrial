pub struct Tally {
    counts: Vec<u64>,
}

impl Tally {
    pub fn new(levels: usize) -> Tally {
        Tally {
            counts: vec![0; levels.max(1)],
        }
    }

    pub fn record(&mut self, level: usize) {
        let slot = self.slot(level);
        self.counts[slot] += 1;
    }

    pub fn count(&self, level: usize) -> u64 {
        self.counts[self.slot(level)]
    }

    pub fn total(&self) -> u64 {
        self.counts.iter().sum()
    }

    pub fn busiest(&self) -> usize {
        (0..self.counts.len())
            .max_by_key(|&level| self.counts[level])
            .unwrap_or(0)
    }

    fn slot(&self, level: usize) -> usize {
        level.min(self.counts.len() - 1)
    }
}
