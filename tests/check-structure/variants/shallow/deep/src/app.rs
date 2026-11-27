use crate::store::Store;

pub fn run(pairs: &[(u32, u32)]) -> u64 {
    let mut store = Store::new(8);
    let kept = store.load(pairs);
    store.summary() + kept as u64
}
