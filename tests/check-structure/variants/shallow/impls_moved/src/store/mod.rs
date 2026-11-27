pub mod table;

pub struct Store {
    pub(crate) table: table::Table,
    pub(crate) limit: usize,
}
