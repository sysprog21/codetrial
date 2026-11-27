use crate::kinds::Stream;

pub fn stream() -> Stream {
    crate::socket::listen(1).unwrap()
}
