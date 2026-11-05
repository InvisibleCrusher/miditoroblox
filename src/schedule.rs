//! Absolute scheduling keeps chords together and avoids cumulative per-note sleeps.
use crate::midi_event::MidiEvent;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub fn deadline(at: Instant, origin: Instant, grid: Duration, ready: Instant) -> Instant {
    let due = if grid.is_zero() {
        at
    } else {
        let nanos = at.saturating_duration_since(origin).as_nanos();
        let step = grid.as_nanos();
        origin + Duration::from_nanos((nanos.div_ceil(step) * step).min(u64::MAX as u128) as u64)
    };
    due.max(ready)
}

pub struct EventQueue {
    origin: Instant,
    pub epoch: u64,
    events: VecDeque<(Instant, MidiEvent)>,
}
impl EventQueue {
    pub fn new(origin: Instant, epoch: u64) -> Self {
        Self {
            origin,
            epoch,
            events: VecDeque::new(),
        }
    }
    /// The caller releases held output before accepting work in the new epoch.
    pub fn transition(&mut self, epoch: u64) -> bool {
        if self.epoch == epoch {
            return false;
        }
        self.events.clear();
        self.epoch = epoch;
        true
    }
    pub fn push(&mut self, event: MidiEvent, grid: Duration, ready: Instant) -> Result<(), ()> {
        if event.epoch != self.epoch {
            return Ok(());
        }
        if self.events.len() >= 4096 {
            self.events.clear();
            return Err(());
        }
        let due = if event.reset {
            event.received_at.max(ready)
        } else {
            deadline(event.received_at, self.origin, grid, ready)
        };
        // Controllers and program changes retain their order across setting changes.
        let due = self
            .events
            .back()
            .map(|(previous, _)| due.max(*previous))
            .unwrap_or(due);
        self.events.push_back((due, event));
        Ok(())
    }
    pub fn timeout(&self, now: Instant) -> Duration {
        self.events
            .front()
            .map(|(due, _)| due.saturating_duration_since(now))
            .unwrap_or(Duration::from_millis(2))
            .min(Duration::from_millis(2))
    }
    pub fn pop_due(&mut self, now: Instant) -> Option<MidiEvent> {
        if self.events.front().is_some_and(|(due, _)| *due <= now) {
            self.events.pop_front().map(|(_, event)| event)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi_event::MidiSource;
    #[test]
    fn quantized_chords_keep_one_deadline_and_original_order() {
        let origin = Instant::now();
        let at = origin + Duration::from_millis(23);
        let mut queue = EventQueue::new(origin, 0);
        for note in [60, 64, 67] {
            let mut event = MidiEvent::new(MidiSource::Player, vec![0x90, note, 100], 0);
            event.received_at = at;
            queue
                .push(event, Duration::from_millis(100), origin)
                .unwrap();
        }
        assert!(queue.pop_due(origin + Duration::from_millis(99)).is_none());
        for note in [60, 64, 67] {
            assert_eq!(
                queue
                    .pop_due(origin + Duration::from_millis(100))
                    .unwrap()
                    .bytes[1],
                note
            );
        }
    }
    #[test]
    fn transport_transition_flushes_old_work_and_rejects_late_old_events() {
        let origin = Instant::now();
        let mut queue = EventQueue::new(origin, 0);
        let old = MidiEvent::at(vec![0x90, 60, 100], 0, origin);
        queue
            .push(
                old.clone(),
                Duration::from_millis(100),
                origin + Duration::from_millis(200),
            )
            .unwrap();
        assert!(queue.transition(1));
        queue.push(old, Duration::ZERO, origin).unwrap();
        assert!(queue.pop_due(origin + Duration::from_secs(1)).is_none());
        queue
            .push(
                MidiEvent::at(vec![0x90, 64, 100], 1, origin),
                Duration::ZERO,
                origin,
            )
            .unwrap();
        assert_eq!(queue.pop_due(origin).unwrap().bytes[1], 64);
    }
    #[test]
    fn focus_grace_is_an_absolute_deadline() {
        let origin = Instant::now();
        let ready = origin + Duration::from_millis(200);
        assert_eq!(deadline(origin, origin, Duration::ZERO, ready), ready);
        let later = origin + Duration::from_millis(300);
        assert_eq!(deadline(later, origin, Duration::ZERO, ready), later);
    }
}
