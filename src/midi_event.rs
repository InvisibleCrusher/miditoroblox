use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MidiSource {
    Live,
    Player,
}

#[derive(Clone, Debug)]
pub struct MidiEvent {
    pub source: MidiSource,
    pub bytes: Vec<u8>,
    pub epoch: u64,
    pub received_at: Instant,
    pub reset: bool,
}

impl MidiEvent {
    pub fn new(source: MidiSource, bytes: Vec<u8>, epoch: u64) -> Self {
        Self {
            source,
            bytes,
            epoch,
            received_at: Instant::now(),
            reset: false,
        }
    }

    pub fn reset(source: MidiSource, epoch: u64, at: Instant) -> Self {
        Self {
            source,
            bytes: Vec::new(),
            epoch,
            received_at: at,
            reset: true,
        }
    }

    pub fn at(bytes: Vec<u8>, epoch: u64, at: Instant) -> Self {
        Self {
            source: MidiSource::Player,
            bytes,
            epoch,
            received_at: at,
            reset: false,
        }
    }

    pub fn allowed(&self, epoch: u64, panic: bool, focused: bool, live_mode: bool) -> bool {
        self.epoch == epoch
            && !panic
            && (live_mode == (self.source == MidiSource::Live))
            && !focused
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gate_rejects_stale_panicked_focused_and_inactive_source_events() {
        let live = MidiEvent::new(MidiSource::Live, vec![], 4);
        assert!(live.allowed(4, false, false, true));
        assert!(!live.allowed(5, false, false, true));
        assert!(!live.allowed(4, true, false, true));
        assert!(!live.allowed(4, false, true, true));
        assert!(!live.allowed(4, false, false, false));
        let player = MidiEvent::new(MidiSource::Player, vec![], 4);
        assert!(player.allowed(4, false, false, false));
        assert!(!player.allowed(4, false, false, true));
        assert!(!player.allowed(4, true, false, false));
        assert!(!player.allowed(4, false, true, false));
    }
}
