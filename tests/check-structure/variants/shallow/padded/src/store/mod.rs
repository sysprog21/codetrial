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

    pub fn limit(&self) -> usize {
        self.limit
    }

    fn table(&self) -> &Table {
        &self.table
    }

    fn table_mut(&mut self) -> &mut Table {
        &mut self.table
    }

    fn count(&self) -> usize {
        self.table.count()
    }

    fn find(&self, key: u32) -> Option<usize> {
        self.table().find(key)
    }

    fn value(&self, at: usize) -> u32 {
        self.table().value(at)
    }

    pub fn insert(&mut self, key: u32, value: u32) -> bool {
        if self.count() >= self.limit() {
            return false;
        }
        self.table_mut().put(key, value);
        true
    }

    pub fn get(&self, key: u32) -> Option<u32> {
        self.find(key).map(|at| self.value(at))
    }

    pub fn remove(&mut self, key: u32) -> Option<u32> {
        let at = self.find(key)?;
        Some(self.table_mut().take(at))
    }

    pub fn contains(&self, key: u32) -> bool {
        self.find(key).is_some()
    }

    pub fn room(&self) -> usize {
        self.limit() - self.count()
    }

    pub fn is_full(&self) -> bool {
        self.room() == 0
    }

    pub fn total(&self) -> u64 {
        (0..self.count()).map(|at| u64::from(self.value(at))).sum()
    }

    pub fn largest(&self) -> Option<u32> {
        (0..self.count()).map(|at| self.value(at)).max()
    }
}
