use std::time::Instant;

#[derive(Debug)]
pub(crate) struct PeerStats {
    times_connected: usize, // TODO: can be NonZeroUsize
    first_seen: Instant,
    last_seen: Instant,
    msg_count: usize,
    bytes_received: u64,
    failures: u8,
}

impl Default for PeerStats {
    fn default() -> Self {
        let now = Instant::now();

        Self {
            times_connected: 1,
            first_seen: now,
            last_seen: now,
            msg_count: 0,
            bytes_received: 0,
            failures: 0,
        }
    }
}

impl PeerStats {
    pub(crate) fn new_connection(&mut self) {
        self.times_connected += 1;
        self.last_seen = Instant::now();
    }

    pub(crate) fn got_message(&mut self, msg_len: usize) {
        self.msg_count += 1;
        self.bytes_received += msg_len as u64;
        self.last_seen = Instant::now();
    }
}
