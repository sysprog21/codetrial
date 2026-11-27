pub struct Table {
    keys: Vec<u32>,
    values: Vec<u32>,
}

impl Table {
    pub fn new() -> Table {
        Table {
            keys: Vec::new(),
            values: Vec::new(),
        }
    }

    pub fn count(&self) -> usize {
        self.keys.len()
    }

    pub fn put(&mut self, key: u32, value: u32) {
        match self.find(key) {
            Some(at) => self.values[at] = value,
            None => {
                self.keys.push(key);
                self.values.push(value);
            }
        }
    }

    pub fn find(&self, key: u32) -> Option<usize> {
        self.keys.iter().position(|&k| k == key)
    }

    pub fn value(&self, at: usize) -> u32 {
        self.values[at]
    }

    pub fn take(&mut self, at: usize) -> u32 {
        self.keys.swap_remove(at);
        self.values.swap_remove(at)
    }
}
