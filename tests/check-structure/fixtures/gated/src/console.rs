pub mod handle {
    pub struct Handle(pub u32);

    pub fn open() -> Handle {
        Handle(super::modes::DEFAULT)
    }
}

pub mod modes {
    pub const DEFAULT: u32 = 0x7;

    pub fn of(handle: &super::handle::Handle) -> u32 {
        handle.0 & DEFAULT
    }
}
