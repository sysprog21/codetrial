use std::collections::VecDeque;

pub struct Window {
    latencies: VecDeque<u32>,
    size: usize,
}

impl Window {
    pub fn new(size: usize) -> Window {
        Window {
            latencies: VecDeque::with_capacity(size),
            size,
        }
    }

    pub fn push(&mut self, latency: u32) {
        if self.latencies.len() == self.size {
            self.latencies.pop_front();
        }
        self.latencies.push_back(latency);
    }

    pub fn mean(&self) -> u32 {
        let sum: u64 = self.latencies.iter().map(|&l| u64::from(l)).sum();
        let mean = sum / self.latencies.len().max(1) as u64;
        u32::try_from(mean).unwrap_or(u32::MAX)
    }

    pub fn worst(&self) -> u32 {
        self.latencies.iter().copied().max().unwrap_or(0)
    }
}
