use crate::kinds::Stream;

pub fn listen(backlog: usize) -> Option<Stream> {
    Stream::new(backlog)
}
