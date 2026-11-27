use crate::kinds::Stream;

include!(concat!(env!("OUT_DIR"), "/limits.rs"));

pub fn listen(backlog: usize) -> Option<Stream> {
    Stream::new(backlog)
}
