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
}

pub fn insert(store: &mut Store, key: u32, value: u32) -> bool {
    if store.table.count() >= store.limit {
        return false;
    }
    store.table.put(key, value);
    true
}

pub fn get(store: &Store, key: u32) -> Option<u32> {
    store.table.find(key).map(|at| store.table.value(at))
}

pub fn remove(store: &mut Store, key: u32) -> Option<u32> {
    let at = store.table.find(key)?;
    Some(store.table.take(at))
}

pub fn contains(store: &Store, key: u32) -> bool {
    store.table.find(key).is_some()
}

pub fn room(store: &Store) -> usize {
    store.limit - store.table.count()
}

pub fn is_full(store: &Store) -> bool {
    room(store) == 0
}

pub fn total(store: &Store) -> u64 {
    (0..store.table.count())
        .map(|at| u64::from(store.table.value(at)))
        .sum()
}

pub fn largest(store: &Store) -> Option<u32> {
    (0..store.table.count())
        .map(|at| store.table.value(at))
        .max()
}
