//! The hints an attempt allows and those given.

/// The hints an attempt allows, and those Jim gave, by item.
#[derive(Debug, Default, Clone)]
pub struct Hints {
    pub given: std::collections::BTreeMap<String, u64>,
    pub allowed: u64,
}

impl Hints {
    /// The hints given on `item`.
    pub fn on(&self, item: &str) -> u64 {
        self.given.get(item).copied().unwrap_or(0)
    }
}
