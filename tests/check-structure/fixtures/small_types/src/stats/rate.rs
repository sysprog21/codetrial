pub struct Rate {
    first: u64,
    last: u64,
    lines: u64,
}

impl Rate {
    pub fn new(first: u64) -> Rate {
        Rate {
            first,
            last: first,
            lines: 0,
        }
    }

    pub fn observe(&mut self, second: u64) {
        self.last = self.last.max(second);
        self.lines += 1;
    }

    pub fn per_second(&self) -> u64 {
        self.lines / self.span()
    }

    fn span(&self) -> u64 {
        self.last - self.first + 1
    }
}
