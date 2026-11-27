use crate::socket::BACKLOG_MOST;

pub struct Stream {
    backlog: usize,
    pending: usize,
}

impl Stream {
    pub fn new(backlog: usize) -> Option<Stream> {
        (backlog <= BACKLOG_MOST).then_some(Stream {
            backlog,
            pending: 0,
        })
    }

    pub fn accept(&mut self) -> bool {
        let room = self.pending < self.backlog;
        if room {
            self.pending += 1;
        }
        room
    }
}
