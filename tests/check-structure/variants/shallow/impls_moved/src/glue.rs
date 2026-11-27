use crate::store::Store;
use crate::store::table::Table;

impl Store {
    pub fn new(limit: usize) -> Store {
        Store {
            table: Table::new(),
            limit,
        }
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    pub fn insert(&mut self, key: u32, value: u32) -> bool {
        if self.table.count() >= self.limit {
            return false;
        }
        self.table.put(key, value);
        true
    }

    pub fn get(&self, key: u32) -> Option<u32> {
        self.table.find(key).map(|at| self.table.value(at))
    }

    pub fn remove(&mut self, key: u32) -> Option<u32> {
        let at = self.table.find(key)?;
        Some(self.table.take(at))
    }

    pub fn contains(&self, key: u32) -> bool {
        self.table.find(key).is_some()
    }

    pub fn room(&self) -> usize {
        self.limit - self.table.count()
    }

    pub fn is_full(&self) -> bool {
        self.room() == 0
    }

    pub fn total(&self) -> u64 {
        (0..self.table.count())
            .map(|at| u64::from(self.table.value(at)))
            .sum()
    }

    pub fn largest(&self) -> Option<u32> {
        (0..self.table.count()).map(|at| self.table.value(at)).max()
    }
}
