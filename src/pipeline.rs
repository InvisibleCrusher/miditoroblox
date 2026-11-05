//! MIDI routing and keyboard ownership, independent of the operating system.
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::instrument::{InstrumentSlot, InstrumentType, match_instrument};
use crate::midi_event::{MidiEvent, MidiSource};
use crate::solver::{KeyCode, MappingGroups, SolveRequest, Solver, get_available_mappings};

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub groups: MappingGroups,
    pub solver: bool,
    pub max_jump: i32,
    pub range: i32,
    pub auto_octave: bool,
    pub experimental: bool,
    pub hold_ctrl: bool,
    pub sustain: bool,
    pub velocity: bool,
    pub routing: bool,
    pub fold_octaves: bool,
    pub slots: [InstrumentSlot; 4],
    pub transpose_delay: Duration,
    pub velocity_delay: Duration,
}

#[derive(Debug)]
pub struct Cancelled;
type Result = std::result::Result<(), Cancelled>;

/// Implementations must check the gate on every key and wait.
/// Releases bypass that gate so interruption cannot leave a held key behind.
pub trait Output {
    fn valid(&self) -> bool;
    fn key(&mut self, key: KeyCode, down: bool) -> Result;
    fn release(&mut self, key: KeyCode);
    fn wait(&mut self, duration: Duration) -> Result;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct NoteId {
    source: MidiSource,
    channel: u8,
    note: u8,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct EventOutcome {
    pub triggered: [bool; 4],
    pub unfolded: [bool; 4],
    pub cutoffs: u64,
    pub transpose_steps: u64,
}

pub struct Pipeline {
    pub outcome: EventOutcome,
    pub solver: Solver,
    voices: HashMap<NoteId, Vec<(KeyCode, u8)>>,
    pressed: HashSet<KeyCode>,
    programs: [[u8; 16]; 2],
    pub last_velocity: Option<KeyCode>,
    return_at: Option<Instant>,
    sustain_active: bool,
    sustain_channels: [[bool; 16]; 2],
}

impl Pipeline {
    pub fn new() -> Self {
        Self {
            outcome: EventOutcome::default(),
            solver: Solver::new(),
            voices: HashMap::new(),
            pressed: HashSet::new(),
            programs: [[0; 16]; 2],
            last_velocity: None,
            return_at: None,
            sustain_active: true,
            sustain_channels: [[false; 16]; 2],
        }
    }

    pub fn output_notes(&self) -> HashSet<u8> {
        self.voices
            .values()
            .flatten()
            .map(|(_, note)| *note)
            .collect()
    }

    pub fn input_notes(&self) -> HashSet<u8> {
        self.voices.keys().map(|id| id.note).collect()
    }

    fn emit(&mut self, io: &mut impl Output, key: KeyCode, down: bool) -> Result {
        // A failed write may have accepted a key-down before reporting failure.
        // Track it before emission so cancellation cleanup still releases it.
        if down {
            self.pressed.insert(key);
        }
        io.key(key, down)?;
        if !down {
            self.pressed.remove(&key);
        }
        Ok(())
    }

    fn tap(&mut self, io: &mut impl Output, key: KeyCode, delay: Duration) -> Result {
        self.emit(io, key, true)?;
        io.wait(delay)?;
        self.emit(io, key, false)
    }

    fn modifiers(&mut self, io: &mut impl Output, shift: bool, ctrl: bool) -> Result {
        for (key, down) in [
            (KeyCode::KEY_LEFTSHIFT, shift),
            (KeyCode::KEY_LEFTCTRL, ctrl),
        ] {
            if self.pressed.contains(&key) != down {
                self.emit(io, key, down)?;
            }
        }
        self.solver.shift_active = shift;
        self.solver.ctrl_active = ctrl;
        Ok(())
    }

    fn transpose(&mut self, io: &mut impl Output, target: i32, delay: Duration) -> Result {
        while self.solver.current_transpose != target {
            let step = if target > self.solver.current_transpose {
                1
            } else {
                -1
            };
            let key = if step > 0 {
                KeyCode::KEY_UP
            } else {
                KeyCode::KEY_DOWN
            };
            self.emit(io, key, true)?;
            // The game acts on key-down, so keep this step even if key-up is interrupted.
            self.solver.current_transpose += step;
            self.outcome.transpose_steps += 1;
            self.emit(io, key, false)?;
            io.wait(delay)?;
        }
        Ok(())
    }

    fn release_note(&mut self, id: NoteId, io: &mut impl Output) {
        if let Some(assignments) = self.voices.remove(&id) {
            for (key, _) in assignments {
                if !self
                    .voices
                    .values()
                    .flatten()
                    .any(|(other, _)| *other == key)
                {
                    io.release(key);
                    self.pressed.remove(&key);
                    self.solver.active_keys.remove(&key);
                }
            }
        }
        if self.solver.active_keys.is_empty() {
            for key in [KeyCode::KEY_LEFTSHIFT, KeyCode::KEY_LEFTCTRL] {
                if self.pressed.remove(&key) {
                    io.release(key);
                }
            }
            self.solver.shift_active = false;
            self.solver.ctrl_active = false;
        }
    }

    pub fn release_all(&mut self, io: &mut impl Output) {
        self.return_at = None;
        for key in self.pressed.drain() {
            io.release(key);
        }
        self.voices.clear();
        self.solver.reset_keys();
        self.last_velocity = None;
    }

    pub fn release_source(&mut self, source: MidiSource, io: &mut impl Output) {
        let ids: Vec<_> = self
            .voices
            .keys()
            .filter(|id| id.source == source)
            .copied()
            .collect();
        for id in ids {
            self.release_note(id, io);
        }
    }

    pub fn reset_source_state(&mut self, source: MidiSource) {
        let index = if source == MidiSource::Live { 0 } else { 1 };
        self.programs[index] = [0; 16];
        self.sustain_channels[index] = [false; 16];
        self.last_velocity = None;
    }

    pub fn process(&mut self, event: &MidiEvent, cfg: &Config, io: &mut impl Output) -> Result {
        self.process_at(event, cfg, io, Instant::now())
    }

    pub(crate) fn process_at(
        &mut self,
        event: &MidiEvent,
        cfg: &Config,
        io: &mut impl Output,
        now: Instant,
    ) -> Result {
        self.outcome = EventOutcome::default();
        // Reject stale queued work without disturbing voices started by newer events.
        // A cancellation *during* output must still release its partially emitted keys.
        if !io.valid() {
            return Err(Cancelled);
        }
        let result = self.process_inner(event, cfg, io, now);
        if result.is_err() {
            self.release_all(io);
        }
        result
    }

    fn process_inner(
        &mut self,
        event: &MidiEvent,
        cfg: &Config,
        io: &mut impl Output,
        now: Instant,
    ) -> Result {
        if !io.valid() {
            return Err(Cancelled);
        }
        if event.reset {
            self.release_source(event.source, io);
            self.reset_source_state(event.source);
            return Ok(());
        }
        let bytes = &event.bytes;
        if bytes.len() < 2 || bytes[0] < 0x80 || bytes[0] >= 0xF0 || bytes[1] > 127 {
            return Ok(());
        }
        let channel = bytes[0] & 0x0F;
        let source = match event.source {
            MidiSource::Live => 0,
            MidiSource::Player => 1,
        };
        match bytes[0] & 0xF0 {
            0xC0 => {
                self.programs[source][channel as usize] = bytes[1];
                return Ok(());
            }
            0x80 | 0x90 | 0xB0 if bytes.len() >= 3 && bytes[2] <= 127 => {}
            _ => return Ok(()),
        }
        let status = bytes[0] & 0xF0;
        if status == 0xB0 {
            if matches!(bytes[1], 120 | 123) {
                let ids: Vec<_> = self
                    .voices
                    .keys()
                    .filter(|id| id.source == event.source && id.channel == channel)
                    .copied()
                    .collect();
                for id in ids {
                    self.release_note(id, io);
                }
            }
            if cfg.sustain && bytes[1] == 64 {
                self.sustain_channels[source][channel as usize] = bytes[2] >= 64;
                let down = self.sustain_channels[source].iter().any(|value| *value);
                if down != self.sustain_active {
                    self.tap(io, KeyCode::KEY_APOSTROPHE, Duration::ZERO)?;
                    self.sustain_active = down;
                }
            }
            return Ok(());
        }
        let id = NoteId {
            source: event.source,
            channel,
            note: bytes[1],
        };
        // A release uses the saved assignment, even after routing or solver settings change.
        self.release_note(id, io);
        if status == 0x80 || bytes[2] == 0 {
            return Ok(());
        }
        self.note_received(cfg.solver, now);
        if channel == 9 && !cfg.routing {
            return Ok(());
        }
        let targets = route(
            cfg,
            channel,
            self.programs[source][channel as usize],
            id.note,
        );
        if targets.is_empty() {
            return Ok(());
        }
        let (min, max) = transpose_bounds(cfg);
        for destination in targets {
            let target = destination.pitch;
            let choice = if cfg.solver {
                self.solver.solve_bounded(SolveRequest {
                    target,
                    max_jump: cfg.max_jump,
                    transpose_bounds: (min, max),
                    physical_range: destination.physical_range,
                    groups: cfg.groups,
                    now,
                })
            } else {
                direct_mapping(target, cfg, destination.physical_range).map(|map| {
                    let transpose = if cfg.experimental && map.shift && !map.ctrl {
                        1
                    } else {
                        0
                    };
                    (transpose, map)
                })
            };
            let Some((transpose, map)) = choice else {
                continue;
            };
            if cfg.velocity {
                let key = crate::solver::key_from_code(
                    crate::platform::VELOCITY_KEYS[((bytes[2] as usize * 31) / 127).min(31)],
                );
                // Keep the current velocity if its control key is holding a note.
                // Do not cache a skipped change: the next note can retry once the key is free.
                if self.last_velocity != Some(key) && !self.pressed.contains(&key) {
                    self.modifiers(io, false, false)?;
                    self.emit(io, KeyCode::KEY_LEFTALT, true)?;
                    io.wait(cfg.velocity_delay)?;
                    self.tap(io, key, cfg.velocity_delay)?;
                    io.wait(cfg.velocity_delay)?;
                    self.emit(io, KeyCode::KEY_LEFTALT, false)?;
                    self.last_velocity = Some(key);
                }
            }
            self.transpose(
                io,
                transpose,
                if cfg.solver {
                    Duration::from_millis(5)
                } else {
                    cfg.transpose_delay
                },
            )?;
            // Share identical pitches across channels; steal only when the physical pitch differs.
            let same_pitch = self
                .solver
                .active_keys
                .get(&map.key_code)
                .is_some_and(|(note, _)| *note == target);
            if !same_pitch {
                self.steal(map.key_code, io)?;
            } else {
                self.emit(io, map.key_code, false)?;
            }
            let shift = map.shift && (cfg.solver || !cfg.experimental);
            self.modifiers(io, shift, map.ctrl)?;
            self.emit(io, map.key_code, true)?;
            if let Some(slot) = destination.slot {
                self.outcome.triggered[slot] = true;
                self.outcome.unfolded[slot] = !destination.folded
                    && if cfg.solver {
                        map.midi_note as i32 + transpose == target as i32
                    } else {
                        map.midi_note == target
                    };
            }
            let held = cfg.solver
                || (map.ctrl && cfg.hold_ctrl)
                || (map.shift && cfg.experimental)
                || (!map.ctrl && !map.shift);
            if held {
                self.voices
                    .entry(id)
                    .or_default()
                    .push((map.key_code, target));
                self.solver
                    .register_note_on(map.key_code, target, transpose, shift, map.ctrl, now);
            } else {
                self.emit(io, map.key_code, false)?;
            }
            if !cfg.solver {
                self.modifiers(io, false, false)?;
            }
        }
        Ok(())
    }

    pub fn note_received(&mut self, enabled: bool, now: Instant) {
        self.return_at = enabled.then_some(now + Duration::from_millis(300));
    }

    pub fn cancel_return(&mut self) {
        self.return_at = None;
    }

    pub fn return_tick(&mut self, cfg: &Config, io: &mut impl Output, now: Instant) -> Result {
        if !cfg.solver || !io.valid() {
            self.cancel_return();
            return Ok(());
        }
        if self.return_at.is_none_or(|at| now < at) {
            return Ok(());
        }
        let current = self.solver.current_transpose;
        if current == 0 {
            self.cancel_return();
            return Ok(());
        }
        let target = current - current.signum();
        let (min, max) = transpose_bounds(cfg);
        if !(min..=max).contains(&target) {
            self.cancel_return();
            return Ok(());
        }
        let (shift, ctrl) = (self.solver.shift_active, self.solver.ctrl_active);
        let result = (|| {
            self.modifiers(io, false, false)?;
            self.transpose(io, target, Duration::ZERO)?;
            self.modifiers(io, shift, ctrl)
        })();
        if result.is_err() {
            self.release_all(io);
        } else {
            self.return_at = Some(now + Duration::from_millis(100));
        }
        result
    }

    pub(crate) fn next_return(&self) -> Option<Instant> {
        self.return_at
    }

    fn steal(&mut self, key: KeyCode, io: &mut impl Output) -> Result {
        self.outcome.cutoffs += self
            .voices
            .values()
            .filter(|assignments| assignments.iter().any(|(k, _)| *k == key))
            .count() as u64;
        if self.pressed.contains(&key) {
            self.emit(io, key, false)?;
            io.wait(Duration::from_millis(5))?;
        }
        for assignments in self.voices.values_mut() {
            assignments.retain(|(k, _)| *k != key);
        }
        self.voices.retain(|_, assignments| !assignments.is_empty());
        self.solver.active_keys.remove(&key);
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Destination {
    pub slot: Option<usize>,
    pub pitch: u8,
    pub physical_range: (u8, u8),
    pub folded: bool,
}

pub(crate) fn route(cfg: &Config, channel: u8, program: u8, note: u8) -> Vec<Destination> {
    if !cfg.routing {
        return vec![Destination {
            slot: None,
            pitch: note,
            physical_range: (21, 108),
            folded: false,
        }];
    }
    let target = |(index, slot): (usize, &InstrumentSlot)| {
        let physical = note as i32 - 20 - slot.transposition as i32;
        fit_pitch(physical, slot.min_note, slot.max_note, cfg.fold_octaves).map(|fitted| {
            Destination {
                slot: Some(index),
                pitch: (fitted + 20) as u8,
                physical_range: (slot.min_note + 20, slot.max_note + 20),
                folded: fitted != physical,
            }
        })
    };
    let mut notes: Vec<_> = cfg
        .slots
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            s.enabled
                && s.inst_type != InstrumentType::Other
                && match_instrument(channel, program, s.inst_type)
        })
        .filter_map(target)
        .collect();
    if notes.is_empty()
        && channel != 9
        && let Some(slot) = cfg
            .slots
            .iter()
            .enumerate()
            .find(|(_, s)| s.enabled && s.inst_type == InstrumentType::Other)
    {
        notes.extend(target(slot));
    }
    // Identical destinations share one assignment; distinct ranges remain separate.
    let mut seen = HashSet::new();
    if cfg.solver {
        notes.retain(|d| seen.insert((d.pitch, d.physical_range)));
    }
    // Non-solver routing retains its previous pitch deduplication and order.
    if !cfg.solver {
        notes.sort_by_key(|d| d.pitch);
        notes.dedup_by_key(|d| d.pitch);
    }
    notes
}

fn transpose_bounds(cfg: &Config) -> (i32, i32) {
    let (mut lo, mut hi) = (-cfg.range, cfg.range);
    if cfg.routing {
        for slot in cfg
            .slots
            .iter()
            .filter(|s| s.enabled && s.inst_type != InstrumentType::Drum)
        {
            lo = lo.max(-24 - slot.transposition as i32);
            hi = hi.min(24 - slot.transposition as i32);
        }
    }
    (lo, hi)
}

/// Fold in physical-key coordinates. The nearest equivalent preserves pitch class.
pub(crate) fn fit_pitch(pitch: i32, min: u8, max: u8, fold: bool) -> Option<i32> {
    let (min, max) = (min as i32, max as i32);
    if (min..=max).contains(&pitch) {
        return Some(pitch);
    }
    if !fold || min > max {
        return None;
    }
    let fitted = if pitch < min {
        pitch + (min - pitch + 11) / 12 * 12
    } else {
        pitch - (pitch - max + 11) / 12 * 12
    };
    (min..=max).contains(&fitted).then_some(fitted)
}

fn direct_mapping(
    note: u8,
    cfg: &Config,
    physical_range: (u8, u8),
) -> Option<crate::solver::KeyMapping> {
    let mappings = get_available_mappings();
    let find = |note| {
        mappings
            .iter()
            .find(|m| {
                m.midi_note == note && cfg.groups.contains(note) && {
                    let physical = note as i32 - i32::from(cfg.experimental && m.shift && !m.ctrl);
                    (physical_range.0 as i32..=physical_range.1 as i32).contains(&physical)
                }
            })
            .copied()
    };
    find(note).or_else(|| {
        if !cfg.auto_octave {
            return None;
        }
        (1..=10)
            .map(|octave| note as i32 + octave * 12)
            .chain((1..=10).map(|octave| note as i32 - octave * 12))
            .filter(|note| (21..=108).contains(note))
            .find_map(|note| find(note as u8))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    enum Action {
        Key(KeyCode, bool),
    }

    struct Recorder {
        actions: Vec<Action>,
        event: MidiEvent,
        epoch: u64,
        panic: bool,
        focused: bool,
        live: bool,
        waits: usize,
        cancel_at_wait: Option<usize>,
        focus_at_wait: Option<usize>,
        panic_at_wait: Option<usize>,
        fail_at_key: Option<usize>,
        key_calls: usize,
    }

    impl Recorder {
        fn new(source: MidiSource) -> Self {
            Self {
                actions: vec![],
                event: MidiEvent::new(source, vec![], 0),
                epoch: 0,
                panic: false,
                focused: false,
                live: source == MidiSource::Live,
                waits: 0,
                cancel_at_wait: None,
                focus_at_wait: None,
                panic_at_wait: None,
                fail_at_key: None,
                key_calls: 0,
            }
        }
        fn held(&self) -> HashSet<KeyCode> {
            let mut keys = HashSet::new();
            for action in &self.actions {
                let Action::Key(key, down) = action;
                if *down {
                    keys.insert(*key);
                } else {
                    keys.remove(key);
                }
            }
            keys
        }
    }

    impl Output for Recorder {
        fn valid(&self) -> bool {
            self.event
                .allowed(self.epoch, self.panic, self.focused, self.live)
        }
        fn key(&mut self, key: KeyCode, down: bool) -> Result {
            if !self.valid() {
                return Err(Cancelled);
            }
            self.actions.push(Action::Key(key, down));
            self.key_calls += 1;
            if self.fail_at_key == Some(self.key_calls) {
                return Err(Cancelled);
            }
            Ok(())
        }
        fn release(&mut self, key: KeyCode) {
            self.actions.push(Action::Key(key, false));
        }
        fn wait(&mut self, _: Duration) -> Result {
            self.waits += 1;
            if self.cancel_at_wait == Some(self.waits) {
                // Panic can already have cleared by the time a delayed operation wakes up.
                self.epoch += 1;
            }
            if self.focus_at_wait == Some(self.waits) {
                self.focused = true;
            }
            if self.panic_at_wait == Some(self.waits) {
                self.panic = true;
            }
            if self.valid() { Ok(()) } else { Err(Cancelled) }
        }
    }

    fn config() -> Config {
        Config {
            groups: MappingGroups {
                low: true,
                middle: true,
                high: true,
            },
            solver: true,
            max_jump: 24,
            range: 0,
            auto_octave: true,
            experimental: true,
            hold_ctrl: true,
            sustain: true,
            velocity: false,
            routing: false,
            fold_octaves: false,
            slots: [InstrumentSlot {
                enabled: false,
                inst_type: InstrumentType::Piano,
                min_note: 1,
                max_note: 88,
                transposition: 0,
            }; 4],
            transpose_delay: Duration::ZERO,
            velocity_delay: Duration::ZERO,
        }
    }

    fn send(p: &mut Pipeline, cfg: &Config, io: &mut Recorder, bytes: &[u8]) {
        let event = MidiEvent::new(io.event.source, bytes.to_vec(), io.event.epoch);
        p.process(&event, cfg, io).unwrap();
    }

    #[test]
    fn range_candidates_stay_in_slot_at_both_edges_and_transpose_directions() {
        for current in [-5, 5] {
            for note in [60, 61, 71, 72] {
                let (mut p, mut cfg, mut io) =
                    (Pipeline::new(), config(), Recorder::new(MidiSource::Live));
                cfg.range = 24;
                cfg.routing = true;
                cfg.slots[0].enabled = true;
                cfg.slots[0].min_note = 40;
                cfg.slots[0].max_note = 52;
                p.solver.current_transpose = current;
                send(&mut p, &cfg, &mut io, &[0x90, note, 100]);
                assert_eq!(p.output_notes(), HashSet::from([note]));
                for (&key, &(pitch, _)) in &p.solver.active_keys {
                    let physical = pitch as i32 - p.solver.current_transpose;
                    assert!((60..=72).contains(&physical));
                    assert!(
                        get_available_mappings()
                            .iter()
                            .any(|m| m.midi_note as i32 == physical && m.key_code == key)
                    );
                }
            }
        }
    }

    #[test]
    fn overlapping_slot_ranges_remain_distinct_but_identical_destinations_share() {
        let mut cfg = config();
        cfg.routing = true;
        cfg.slots[0].enabled = true;
        cfg.slots[1] = cfg.slots[0];
        assert_eq!(route(&cfg, 0, 0, 60).len(), 1);
        cfg.slots[1].min_note = 40;
        let routes = route(&cfg, 0, 0, 60);
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].slot, Some(0));
        assert_eq!(routes[1].slot, Some(1));
        assert_ne!(routes[0].physical_range, routes[1].physical_range);
    }

    #[test]
    fn return_is_spaced_converges_both_directions_and_never_catches_up() {
        let start = Instant::now();
        for initial in [-5, 5] {
            let (mut p, mut cfg, mut io) =
                (Pipeline::new(), config(), Recorder::new(MidiSource::Live));
            cfg.range = 24;
            p.solver.current_transpose = initial;
            p.note_received(true, start);
            p.return_tick(&cfg, &mut io, start + Duration::from_millis(299))
                .unwrap();
            assert_eq!(p.solver.current_transpose, initial);
            p.return_tick(&cfg, &mut io, start + Duration::from_millis(300))
                .unwrap();
            assert_eq!(p.solver.current_transpose.abs(), 4);
            p.return_tick(&cfg, &mut io, start + Duration::from_millis(399))
                .unwrap();
            assert_eq!(p.solver.current_transpose.abs(), 4);
            // A late tick moves once, then starts a fresh interval.
            p.return_tick(&cfg, &mut io, start + Duration::from_secs(2))
                .unwrap();
            assert_eq!(p.solver.current_transpose.abs(), 3);
            p.return_tick(&cfg, &mut io, start + Duration::from_secs(2))
                .unwrap();
            assert_eq!(p.solver.current_transpose.abs(), 3);
            for ms in [2100, 2200, 2300, 2400] {
                p.return_tick(&cfg, &mut io, start + Duration::from_millis(ms))
                    .unwrap();
            }
            assert_eq!(p.solver.current_transpose, 0);
            assert!(p.return_at.is_none());
            assert!(io.held().is_empty());
        }
    }

    #[test]
    fn skipped_note_restarts_return_but_controllers_and_note_off_do_not() {
        let start = Instant::now();
        let (mut p, mut cfg, mut io) = (Pipeline::new(), config(), Recorder::new(MidiSource::Live));
        cfg.range = 24;
        cfg.max_jump = 3;
        cfg.groups = MappingGroups {
            low: false,
            middle: false,
            high: false,
        };
        p.solver.current_transpose = -5;
        p.note_received(true, start);
        let on = MidiEvent::new(MidiSource::Live, vec![0x90, 60, 100], 0);
        p.process_at(&on, &cfg, &mut io, start + Duration::from_millis(250))
            .unwrap();
        assert!(p.voices.is_empty());
        for bytes in [vec![0x80, 60, 0], vec![0xB0, 1, 0], vec![0x90, 60, 0]] {
            let event = MidiEvent::new(MidiSource::Live, bytes, 0);
            p.process_at(&event, &cfg, &mut io, start + Duration::from_millis(400))
                .unwrap();
        }
        p.return_tick(&cfg, &mut io, start + Duration::from_millis(549))
            .unwrap();
        assert_eq!(p.solver.current_transpose, -5);
        p.return_tick(&cfg, &mut io, start + Duration::from_millis(550))
            .unwrap();
        assert_eq!(p.solver.current_transpose, -4);
    }

    #[test]
    fn return_restores_modifiers_and_saved_owners_release_held_notes() {
        let start = Instant::now();
        let (mut p, mut cfg, mut io) = (Pipeline::new(), config(), Recorder::new(MidiSource::Live));
        cfg.range = 24;
        // Two channels share a low Ctrl note.
        send(&mut p, &cfg, &mut io, &[0x90, 21, 100]);
        send(&mut p, &cfg, &mut io, &[0x91, 21, 100]);
        p.solver.current_transpose = -5;
        p.note_received(true, start);
        let held = io.held();
        let before = io.actions.len();
        p.return_tick(&cfg, &mut io, start + Duration::from_millis(300))
            .unwrap();
        assert_eq!(io.held(), held);
        assert_eq!(
            &io.actions[before..],
            &[
                Action::Key(KeyCode::KEY_LEFTCTRL, false),
                Action::Key(KeyCode::KEY_UP, true),
                Action::Key(KeyCode::KEY_UP, false),
                Action::Key(KeyCode::KEY_LEFTCTRL, true),
            ]
        );
        send(&mut p, &cfg, &mut io, &[0x80, 21, 0]);
        assert!(io.held().contains(&KeyCode::KEY_1));
        send(&mut p, &cfg, &mut io, &[0x81, 21, 0]);
        assert!(io.held().is_empty());
    }

    #[test]
    fn return_cancellation_and_partial_key_failures_leave_no_orphans() {
        let start = Instant::now();
        for failure in 1..=4 {
            let (mut p, mut cfg, mut io) =
                (Pipeline::new(), config(), Recorder::new(MidiSource::Live));
            cfg.range = 24;
            send(&mut p, &cfg, &mut io, &[0x90, 21, 100]);
            p.solver.current_transpose = -5;
            p.note_received(true, start);
            io.fail_at_key = Some(io.key_calls + failure);
            assert!(
                p.return_tick(&cfg, &mut io, start + Duration::from_millis(300))
                    .is_err()
            );
            assert!(io.held().is_empty());
            assert!(p.voices.is_empty());
            assert!(p.return_at.is_none());
        }
        for gate in 0..5 {
            let (mut p, mut cfg, mut io) =
                (Pipeline::new(), config(), Recorder::new(MidiSource::Live));
            cfg.range = 24;
            p.solver.current_transpose = -5;
            p.note_received(true, start);
            match gate {
                0 => cfg.solver = false,
                1 => io.focused = true,
                2 => io.panic = true,
                3 => io.epoch += 1,
                _ => io.live = false,
            }
            p.return_tick(&cfg, &mut io, start + Duration::from_millis(300))
                .unwrap();
            assert!(io.actions.is_empty());
            assert!(p.return_at.is_none());
        }
    }

    #[test]
    fn controllers_and_non_note_messages_do_not_release_notes() {
        let (mut p, cfg, mut io) = (Pipeline::new(), config(), Recorder::new(MidiSource::Live));
        send(&mut p, &cfg, &mut io, &[0x90, 60, 100]);
        let count = io.actions.len();
        for bytes in [
            &[0xB0, 60, 127][..],
            &[0xE0, 60, 0],
            &[0xA0, 60, 0],
            &[0xD0, 60],
            &[0x90],
            &[0x90, 200, 0],
        ] {
            send(&mut p, &cfg, &mut io, bytes);
        }
        assert_eq!(io.actions.len(), count);
        assert!(io.held().contains(&KeyCode::KEY_T));
        send(&mut p, &cfg, &mut io, &[0x90, 60, 0]);
        assert!(io.held().is_empty());
    }

    #[test]
    fn overlapping_channels_share_a_pitch_until_both_release() {
        let (mut p, cfg, mut io) = (Pipeline::new(), config(), Recorder::new(MidiSource::Live));
        send(&mut p, &cfg, &mut io, &[0x90, 60, 100]);
        send(&mut p, &cfg, &mut io, &[0x91, 60, 100]);
        send(&mut p, &cfg, &mut io, &[0x80, 60, 0]);
        assert!(io.held().contains(&KeyCode::KEY_T));
        send(&mut p, &cfg, &mut io, &[0x81, 60, 0]);
        assert!(io.held().is_empty());
    }

    #[test]
    fn sources_have_independent_ownership() {
        let (mut p, cfg, mut io) = (Pipeline::new(), config(), Recorder::new(MidiSource::Live));
        send(&mut p, &cfg, &mut io, &[0x90, 60, 100]);
        io.event.source = MidiSource::Player;
        io.live = false;
        send(&mut p, &cfg, &mut io, &[0x90, 60, 100]);
        send(&mut p, &cfg, &mut io, &[0x80, 60, 0]);
        assert!(io.held().contains(&KeyCode::KEY_T));
        p.release_source(MidiSource::Live, &mut io);
        assert!(io.held().is_empty());
    }

    #[test]
    fn stolen_key_is_not_released_by_its_previous_owner() {
        let (mut p, cfg, mut io) = (Pipeline::new(), config(), Recorder::new(MidiSource::Live));
        send(&mut p, &cfg, &mut io, &[0x90, 36, 100]);
        send(&mut p, &cfg, &mut io, &[0x91, 37, 100]);
        send(&mut p, &cfg, &mut io, &[0x80, 36, 0]);
        assert!(io.held().contains(&KeyCode::KEY_1));
        assert_eq!(p.output_notes(), HashSet::from([37]));
        send(&mut p, &cfg, &mut io, &[0x81, 37, 0]);
        assert!(io.held().is_empty());
    }

    #[test]
    fn note_off_uses_saved_routes_after_settings_and_program_change() {
        for solver in [false, true] {
            let (mut p, mut cfg, mut io) =
                (Pipeline::new(), config(), Recorder::new(MidiSource::Live));
            cfg.solver = solver;
            cfg.routing = true;
            cfg.slots[0].enabled = true;
            cfg.slots[0].transposition = 12;
            send(&mut p, &cfg, &mut io, &[0x90, 72, 100]);
            assert!(io.held().contains(&KeyCode::KEY_T));
            cfg.slots[0].enabled = false;
            cfg.groups.middle = false;
            cfg.solver = !solver;
            send(&mut p, &cfg, &mut io, &[0xC0, 40]);
            send(&mut p, &cfg, &mut io, &[0x80, 72, 0]);
            assert!(io.held().is_empty());
        }
    }

    #[test]
    fn all_slot_targets_are_processed() {
        for solver in [false, true] {
            let (mut p, mut cfg, mut io) =
                (Pipeline::new(), config(), Recorder::new(MidiSource::Player));
            cfg.solver = solver;
            cfg.routing = true;
            cfg.slots[0].enabled = true;
            cfg.slots[1].enabled = true;
            cfg.slots[1].transposition = 12;
            send(&mut p, &cfg, &mut io, &[0x90, 60, 100]);
            assert_eq!(p.output_notes(), HashSet::from([48, 60]));
            send(&mut p, &cfg, &mut io, &[0x80, 60, 0]);
            assert!(io.held().is_empty());
        }
    }

    #[test]
    fn other_is_fallback_and_never_claims_drums() {
        let mut cfg = config();
        cfg.routing = true;
        cfg.slots[0].enabled = true;
        cfg.slots[0].inst_type = InstrumentType::Other;
        cfg.slots[0].transposition = 12;
        assert_eq!(
            route(&cfg, 0, 0, 60)
                .iter()
                .map(|d| d.pitch)
                .collect::<Vec<_>>(),
            vec![48]
        );
        assert!(route(&cfg, 9, 0, 60).is_empty());
        cfg.slots[1].enabled = true;
        assert_eq!(
            route(&cfg, 0, 0, 60)
                .iter()
                .map(|d| d.pitch)
                .collect::<Vec<_>>(),
            vec![60]
        );
        cfg.slots[1].min_note = 80;
        assert_eq!(
            route(&cfg, 0, 0, 60)
                .iter()
                .map(|d| d.pitch)
                .collect::<Vec<_>>(),
            vec![48]
        );
    }

    #[test]
    fn both_paths_apply_combined_transpose_bounds() {
        for solver in [false, true] {
            let (mut p, mut cfg, mut io) =
                (Pipeline::new(), config(), Recorder::new(MidiSource::Player));
            cfg.solver = solver;
            cfg.range = 24;
            cfg.routing = true;
            cfg.slots[0].enabled = true;
            cfg.slots[0].transposition = 24;
            cfg.slots[1].enabled = true;
            cfg.slots[1].transposition = -24;
            assert_eq!(transpose_bounds(&cfg), (0, 0));
            send(&mut p, &cfg, &mut io, &[0x90, 60, 100]);
            assert_eq!(p.solver.current_transpose, 0);
        }
    }

    #[test]
    fn solver_respects_each_mapping_group() {
        for (note, groups) in [
            (
                21,
                MappingGroups {
                    low: false,
                    middle: true,
                    high: true,
                },
            ),
            (
                60,
                MappingGroups {
                    low: true,
                    middle: false,
                    high: true,
                },
            ),
            (
                108,
                MappingGroups {
                    low: true,
                    middle: true,
                    high: false,
                },
            ),
        ] {
            let (mut p, mut cfg, mut io) =
                (Pipeline::new(), config(), Recorder::new(MidiSource::Live));
            cfg.groups = groups;
            send(&mut p, &cfg, &mut io, &[0x90, note, 100]);
            assert!(io.actions.is_empty());
        }
        let solver = Solver::new();
        let groups = MappingGroups {
            low: false,
            middle: true,
            high: false,
        };
        let (_, mapping) = solver
            .solve_bounded(SolveRequest {
                target: 30,
                max_jump: 24,
                transpose_bounds: (-24, 24),
                physical_range: (21, 108),
                groups,
                now: Instant::now(),
            })
            .unwrap();
        assert!(groups.contains(mapping.midi_note));
    }

    #[test]
    fn focused_live_batch_emits_no_keys() {
        let (mut p, cfg, mut io) = (Pipeline::new(), config(), Recorder::new(MidiSource::Live));
        io.focused = true;
        for bytes in [vec![0x99, 60, 100], vec![0x90, 64, 100]] {
            let event = MidiEvent::new(MidiSource::Live, bytes, 0);
            assert!(p.process(&event, &cfg, &mut io).is_err());
        }
        assert!(io.actions.is_empty());
    }

    #[test]
    fn interruption_at_every_delay_releases_partial_output_and_rejects_stale_work() {
        // Includes velocity modifier holds and transpose taps.
        let mut cfg = config();
        cfg.velocity = true;
        cfg.range = 24;
        let mut probe = Recorder::new(MidiSource::Live);
        send(&mut Pipeline::new(), &cfg, &mut probe, &[0x90, 10, 100]);
        for wait in 1..=probe.waits {
            let (mut p, mut io) = (Pipeline::new(), Recorder::new(MidiSource::Live));
            io.cancel_at_wait = Some(wait);
            let event = MidiEvent::new(MidiSource::Live, vec![0x90, 10, 100], 0);
            assert!(p.process(&event, &cfg, &mut io).is_err(), "wait {wait}");
            assert!(io.held().is_empty(), "wait {wait}");
            let count = io.actions.len();
            assert!(p.process(&event, &cfg, &mut io).is_err());
            assert_eq!(io.actions.len(), count);
        }
    }
    #[test]
    fn focus_and_panic_during_a_delay_release_existing_notes() {
        for panic in [false, true] {
            let (mut p, mut cfg, mut io) =
                (Pipeline::new(), config(), Recorder::new(MidiSource::Live));
            send(&mut p, &cfg, &mut io, &[0x90, 60, 100]);
            if panic {
                io.panic_at_wait = Some(io.waits + 1);
            } else {
                io.focus_at_wait = Some(io.waits + 1);
            }
            cfg.velocity = true;
            let event = MidiEvent::new(MidiSource::Live, vec![0x90, 64, 100], 0);
            assert!(p.process(&event, &cfg, &mut io).is_err());
            assert!(io.held().is_empty());
            assert!(p.output_notes().is_empty());
        }
    }

    #[test]
    fn repeated_note_on_and_duplicate_slot_targets_leave_no_orphans() {
        let (mut p, mut cfg, mut io) =
            (Pipeline::new(), config(), Recorder::new(MidiSource::Player));
        cfg.routing = true;
        cfg.slots[0].enabled = true;
        cfg.slots[1].enabled = true;
        for _ in 0..3 {
            send(&mut p, &cfg, &mut io, &[0x90, 60, 100]);
        }
        assert_eq!(p.voices.values().flatten().count(), 1);
        send(&mut p, &cfg, &mut io, &[0x80, 60, 0]);
        assert!(io.held().is_empty());
    }
    #[test]
    fn stale_or_inactive_source_events_do_not_release_newer_voices() {
        let (mut p, cfg, mut io) = (Pipeline::new(), config(), Recorder::new(MidiSource::Live));
        send(&mut p, &cfg, &mut io, &[0x90, 60, 100]);
        let actions = io.actions.len();
        io.epoch = 1;
        let stale = MidiEvent::new(MidiSource::Live, vec![0x80, 60, 0], 0);
        assert!(p.process(&stale, &cfg, &mut io).is_err());
        assert_eq!(io.actions.len(), actions);
        assert!(io.held().contains(&KeyCode::KEY_T));
        io.epoch = 0;
        io.event.source = MidiSource::Player;
        let inactive = MidiEvent::new(MidiSource::Player, vec![0x80, 60, 0], 0);
        assert!(p.process(&inactive, &cfg, &mut io).is_err());
        assert_eq!(io.actions.len(), actions);
    }
    #[test]
    fn velocity_collision_preserves_held_notes_and_retries_when_free() {
        for solver in [false, true] {
            for source in [MidiSource::Live, MidiSource::Player] {
                let (mut p, mut cfg, mut io) = (Pipeline::new(), config(), Recorder::new(source));
                cfg.solver = solver;
                cfg.velocity = true;
                send(&mut p, &cfg, &mut io, &[0x90, 60, 100]); // C4 on T
                let previous_velocity = p.last_velocity;
                let start = io.actions.len();
                send(&mut p, &cfg, &mut io, &[0x91, 64, 58]); // E4; velocity wants Alt+T
                assert_eq!(p.output_notes(), HashSet::from([60, 64]));
                assert!(io.held().contains(&KeyCode::KEY_T));
                assert_eq!(
                    p.last_velocity, previous_velocity,
                    "a skipped change is not cached"
                );
                assert!(!io.actions[start..].iter().any(|action| matches!(
                    action,
                    Action::Key(KeyCode::KEY_T, _) | Action::Key(KeyCode::KEY_LEFTALT, _)
                )));

                send(&mut p, &cfg, &mut io, &[0x80, 60, 0]);
                assert!(io.held().contains(&KeyCode::KEY_U));
                let start = io.actions.len();
                send(&mut p, &cfg, &mut io, &[0x92, 67, 58]); // T is free; retry the same velocity
                assert_eq!(p.last_velocity, Some(KeyCode::KEY_T));
                assert_eq!(p.output_notes(), HashSet::from([64, 67]));
                let mut alt_down = false;
                let mut taps = 0;
                for action in &io.actions[start..] {
                    match action {
                        Action::Key(KeyCode::KEY_LEFTALT, down) => alt_down = *down,
                        Action::Key(KeyCode::KEY_T, down) => {
                            assert!(alt_down, "velocity keys must stay inside Alt");
                            if *down {
                                taps += 1;
                            }
                        }
                        _ => {}
                    }
                }
                assert_eq!(taps, 1);
                assert!(!alt_down);
                send(&mut p, &cfg, &mut io, &[0x81, 64, 0]);
                send(&mut p, &cfg, &mut io, &[0x82, 67, 0]);
                assert!(io.held().is_empty());
            }
        }
    }

    #[test]
    fn velocity_collisions_match_disabled_velocity_for_every_mapped_control_key() {
        let mut checked_keys = HashSet::new();
        for solver in [false, true] {
            for mapping in get_available_mappings() {
                let Some(velocity) = (1u8..=127).find(|v| {
                    crate::platform::VELOCITY_KEYS[*v as usize * 31 / 127]
                        == mapping.key_code.code()
                }) else {
                    continue;
                };
                checked_keys.insert(mapping.key_code);
                let incoming = if mapping.key_code == KeyCode::KEY_T {
                    64
                } else {
                    60
                };
                let mut expected = None;
                for velocity_enabled in [false, true] {
                    let (mut p, mut cfg, mut io) =
                        (Pipeline::new(), config(), Recorder::new(MidiSource::Live));
                    cfg.solver = solver;
                    send(&mut p, &cfg, &mut io, &[0x90, mapping.midi_note, 100]);
                    let start = io.actions.len();
                    cfg.velocity = velocity_enabled;
                    send(&mut p, &cfg, &mut io, &[0x91, incoming, velocity]);
                    let result = (io.actions[start..].to_vec(), p.output_notes(), io.held());
                    if let Some(expected) = &expected {
                        assert_eq!(
                            &result, expected,
                            "note {}, velocity {}, solver {}",
                            mapping.midi_note, velocity, solver
                        );
                    } else {
                        expected = Some(result);
                    }
                }
            }
        }
        assert_eq!(checked_keys.len(), crate::platform::VELOCITY_KEYS.len());
    }

    #[test]
    fn a_new_song_resets_programs_before_routing_notes() {
        let (mut p, mut cfg, mut io) =
            (Pipeline::new(), config(), Recorder::new(MidiSource::Player));
        cfg.routing = true;
        cfg.slots[0].enabled = true;
        cfg.slots[1].enabled = true;
        cfg.slots[1].inst_type = InstrumentType::Strings;
        cfg.slots[1].transposition = 12;
        send(&mut p, &cfg, &mut io, &[0xC0, 40]);
        send(&mut p, &cfg, &mut io, &[0x90, 72, 100]);
        assert_eq!(p.output_notes(), HashSet::from([60]));
        let reset = MidiEvent::reset(MidiSource::Player, 0, std::time::Instant::now());
        p.process(&reset, &cfg, &mut io).unwrap();
        assert!(io.held().is_empty());
        send(&mut p, &cfg, &mut io, &[0x90, 72, 100]);
        assert_eq!(p.output_notes(), HashSet::from([72]));
    }

    #[test]
    fn black_key_transpose_stays_optimized_until_the_next_pitch_needs_a_change() {
        let (mut p, mut cfg, mut io) = (Pipeline::new(), config(), Recorder::new(MidiSource::Live));
        cfg.solver = false;
        send(&mut p, &cfg, &mut io, &[0x90, 61, 100]);
        send(&mut p, &cfg, &mut io, &[0x80, 61, 0]);
        assert_eq!(p.solver.current_transpose, 1);
        assert!(!io.actions.contains(&Action::Key(KeyCode::KEY_DOWN, true)));
        send(&mut p, &cfg, &mut io, &[0x90, 63, 100]);
        assert_eq!(
            io.actions
                .iter()
                .filter(|a| **a == Action::Key(KeyCode::KEY_UP, true))
                .count(),
            1
        );
        send(&mut p, &cfg, &mut io, &[0x90, 60, 100]);
        assert_eq!(p.solver.current_transpose, 0);
        assert_eq!(
            io.actions
                .iter()
                .filter(|a| **a == Action::Key(KeyCode::KEY_DOWN, true))
                .count(),
            1
        );
    }

    #[test]
    fn sustain_channels_do_not_cancel_each_other_and_all_notes_off_releases_only_its_channel() {
        let (mut p, cfg, mut io) = (Pipeline::new(), config(), Recorder::new(MidiSource::Player));
        send(&mut p, &cfg, &mut io, &[0xB0, 64, 127]);
        let count = io.actions.len();
        send(&mut p, &cfg, &mut io, &[0xB1, 64, 0]);
        assert_eq!(io.actions.len(), count);
        send(&mut p, &cfg, &mut io, &[0xB0, 64, 0]);
        assert!(
            io.actions
                .contains(&Action::Key(KeyCode::KEY_APOSTROPHE, true))
        );
        send(&mut p, &cfg, &mut io, &[0x90, 60, 100]);
        send(&mut p, &cfg, &mut io, &[0x91, 64, 100]);
        send(&mut p, &cfg, &mut io, &[0xB0, 123, 0]);
        assert_eq!(p.output_notes(), HashSet::from([64]));
        send(&mut p, &cfg, &mut io, &[0xB1, 123, 0]);
        assert!(io.held().is_empty());
    }

    #[test]
    fn quantized_output_and_transport_cancellation_work_together() {
        use crate::schedule::EventQueue;
        use std::time::Instant;
        let origin = Instant::now();
        let mut queue = EventQueue::new(origin, 0);
        let (mut p, cfg, mut io) = (Pipeline::new(), config(), Recorder::new(MidiSource::Player));
        for note in [60, 64, 67] {
            queue
                .push(
                    MidiEvent::at(vec![0x90, note, 100], 0, origin + Duration::from_millis(23)),
                    Duration::from_millis(100),
                    origin,
                )
                .unwrap();
        }
        assert!(queue.pop_due(origin + Duration::from_millis(99)).is_none());
        while let Some(event) = queue.pop_due(origin + Duration::from_millis(100)) {
            p.process(&event, &cfg, &mut io).unwrap();
        }
        assert_eq!(p.output_notes(), HashSet::from([60, 64, 67]));
        queue
            .push(
                MidiEvent::at(vec![0x90, 72, 100], 0, origin + Duration::from_millis(123)),
                Duration::from_millis(100),
                origin,
            )
            .unwrap();
        io.epoch = 1;
        io.event.epoch = 1;
        if queue.transition(1) {
            p.release_all(&mut io);
        }
        assert!(io.held().is_empty());
        assert!(queue.pop_due(origin + Duration::from_secs(1)).is_none());
        queue
            .push(
                MidiEvent::at(vec![0x90, 72, 100], 1, origin),
                Duration::ZERO,
                origin,
            )
            .unwrap();
        let event = queue.pop_due(origin).unwrap();
        p.process(&event, &cfg, &mut io).unwrap();
        assert_eq!(p.output_notes(), HashSet::from([72]));
    }
    #[test]
    fn octave_fitting_preserves_pitch_class_and_uses_nearest_equivalent() {
        for pitch in -43..=151 {
            for min in 1u8..=88 {
                for max in min..=88 {
                    let fitted = fit_pitch(pitch, min, max, true);
                    let expected = (min as i32..=max as i32)
                        .filter(|key| (key - pitch).rem_euclid(12) == 0)
                        .min_by_key(|key| (key - pitch).abs());
                    assert_eq!(fitted, expected);
                    assert_eq!(
                        fit_pitch(pitch, min, max, false),
                        (min as i32..=max as i32).contains(&pitch).then_some(pitch)
                    );
                }
            }
        }
    }

    #[test]
    fn folded_routes_compensate_transpose_and_note_off_keeps_saved_assignment() {
        for solver in [false, true] {
            for transpose in [-24, -1, 0, 1, 24] {
                let mut cfg = config();
                cfg.routing = true;
                cfg.solver = solver;
                cfg.range = 24;
                cfg.fold_octaves = true;
                cfg.slots[0].enabled = true;
                cfg.slots[0].min_note = 40;
                cfg.slots[0].max_note = 52;
                cfg.slots[0].transposition = transpose;
                let note = (84i32 + transpose as i32) as u8;
                let targets = route(&cfg, 0, 0, note);
                assert_eq!(targets.len(), 1);
                assert_eq!(targets[0].pitch, 72);
                assert!(targets[0].folded);
                let (mut p, mut io) = (Pipeline::new(), Recorder::new(MidiSource::Player));
                send(&mut p, &cfg, &mut io, &[0x90, note, 100]);
                assert!(p.outcome.triggered[0]);
                assert!(!p.outcome.unfolded[0]);
                cfg.slots[0].enabled = false;
                send(&mut p, &cfg, &mut io, &[0xc0, 32]);
                send(&mut p, &cfg, &mut io, &[0x90, note, 0]);
                assert!(io.held().is_empty());
            }
        }
    }

    #[test]
    fn folding_typed_slot_precedes_other_and_handles_midi_extremes() {
        let mut cfg = config();
        cfg.routing = true;
        cfg.fold_octaves = true;
        cfg.slots[0].enabled = true;
        cfg.slots[0].min_note = 40;
        cfg.slots[0].max_note = 52;
        cfg.slots[1] = cfg.slots[0];
        cfg.slots[1].enabled = true;
        cfg.slots[1].inst_type = InstrumentType::Other;
        cfg.slots[1].min_note = 1;
        cfg.slots[1].max_note = 88;
        for note in [0, 48, 84, 127] {
            assert_eq!(route(&cfg, 0, 0, note)[0].slot, Some(0));
        }
        cfg.slots[0].max_note = 40;
        assert_eq!(route(&cfg, 0, 0, 61)[0].slot, Some(1));
        cfg.slots[1].enabled = false;
        assert!(route(&cfg, 0, 0, 61).is_empty());
        cfg.fold_octaves = false;
        assert!(route(&cfg, 0, 0, 84).is_empty());
        assert!(route(&cfg, 9, 0, 84).is_empty());
    }

    #[test]
    fn direct_octave_mapping_cannot_escape_destination_range() {
        let mut cfg = config();
        cfg.solver = false;
        cfg.routing = true;
        cfg.slots[0].enabled = true;
        cfg.slots[0].min_note = 1;
        cfg.slots[0].max_note = 15;
        cfg.groups.low = false;
        let (mut p, mut io) = (Pipeline::new(), Recorder::new(MidiSource::Player));
        send(&mut p, &cfg, &mut io, &[0x90, 30, 100]);
        assert!(io.actions.is_empty());
        assert!(!p.outcome.triggered[0]);
        // A suppressed black-key modifier uses the lower white key physically.
        cfg.slots[0].min_note = 17;
        cfg.slots[0].max_note = 17;
        cfg.auto_octave = false;
        send(&mut p, &cfg, &mut io, &[0x90, 37, 100]);
        assert!(io.actions.is_empty());
    }
}
