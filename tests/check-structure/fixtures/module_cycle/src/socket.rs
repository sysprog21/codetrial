use crate::kinds::Stream;

pub const BACKLOG_MOST: usize = 16;

pub fn listen(backlog: usize) -> Option<Stream> {
    Stream::new(backlog)
}
