use crate::store::{self, Store};

pub fn run(pairs: &[(u32, u32)]) -> u64 {
    let mut held = Store::new(8);
    for &(key, value) in pairs {
        if store::is_full(&held) || !store::insert(&mut held, key, value) {
            store::remove(&mut held, key);
        }
    }
    let kept = pairs
        .iter()
        .filter(|&&(key, _)| store::contains(&held, key))
        .count();
    let found = pairs
        .iter()
        .filter_map(|&(key, _)| store::get(&held, key))
        .count();
    let rest = kept + found + store::room(&held) + held.limit();
    store::total(&held) + store::largest(&held).map_or(0, u64::from) + rest as u64
}
