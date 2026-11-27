pub mod table;

use table::Table;

pub struct Store {
    table: Table,
    limit: usize,
}

impl Store {
    pub fn new(limit: usize) -> Store {
        Store {
            table: Table::new(),
            limit,
        }
    }

    /// Takes each pair it has room for, replacing a key's value; when full, a
    /// pair evicts its key. Returns how many of the keys it holds after.
    pub fn load(&mut self, pairs: &[(u32, u32)]) -> usize {
        for &(key, value) in pairs {
            if self.is_full() || !self.insert(key, value) {
                self.remove(key);
            }
        }
        pairs.iter().filter(|&&(key, _)| self.contains(key)).count()
    }

    /// The sum and the largest of the values it holds, and its room left.
    pub fn summary(&self) -> u64 {
        let found = (0..self.table.count()).filter(|&at| self.get(self.table.value(at)).is_some());
        self.total() + self.largest().map_or(0, u64::from) + (found.count() + self.room()) as u64
    }

    fn insert(&mut self, key: u32, value: u32) -> bool {
        if self.table.count() >= self.limit {
            return false;
        }
        self.table.put(key, value);
        true
    }

    fn get(&self, key: u32) -> Option<u32> {
        self.table.find(key).map(|at| self.table.value(at))
    }

    fn remove(&mut self, key: u32) -> Option<u32> {
        let at = self.table.find(key)?;
        Some(self.table.take(at))
    }

    fn contains(&self, key: u32) -> bool {
        self.table.find(key).is_some()
    }

    fn room(&self) -> usize {
        self.limit - self.table.count()
    }

    fn is_full(&self) -> bool {
        self.room() == 0
    }

    fn total(&self) -> u64 {
        (0..self.table.count())
            .map(|at| u64::from(self.table.value(at)))
            .sum()
    }

    fn largest(&self) -> Option<u32> {
        (0..self.table.count()).map(|at| self.table.value(at)).max()
    }
}
