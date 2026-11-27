pub mod counter {
    pub struct Count(pub u64);

    pub fn start() -> Count {
        Count(super::sink::FLOOR)
    }
}

pub mod sink {
    pub const FLOOR: u64 = 0;

    pub fn flush(count: &super::counter::Count) -> u64 {
        count.0.max(FLOOR)
    }
}
