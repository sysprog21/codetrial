use crate::store::Store;

pub fn run(pairs: &[(u32, u32)]) -> u64 {
    let mut store = Store::new(8);
    for &(key, value) in pairs {
        if store.is_full() || !store.insert(key, value) {
            store.remove(key);
        }
    }
    let kept = pairs.iter().filter(|&&(key, _)| store.contains(key)).count();
    let found = pairs.iter().filter_map(|&(key, _)| store.get(key)).count();
    let rest = kept + found + store.room() + store.limit;
    store.total() + store.largest().map_or(0, u64::from) + rest as u64
}
