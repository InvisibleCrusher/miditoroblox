//! Bounded layout search and hardware-free replay of the playback pipeline.
use crate::instrument::{InstrumentSlot, InstrumentType, match_instrument};
use crate::midi_event::MidiEvent;
#[cfg(test)]
use crate::midi_player::analysis_events;
use crate::midi_player::{AnalysisMidi, Mutes, analysis_events_edited};
use crate::pipeline::{Cancelled, Config, Output, Pipeline, route};
use crate::schedule::deadline;
use crate::solver::KeyCode;
use std::cmp::Ordering as Cmp;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

const KEEP: usize = 32;
const LIMIT: usize = 1000;
type Song = AnalysisMidi;

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub content_hash: Option<String>,
    pub mutes: Mutes,
    pub path: PathBuf,
    pub config: Config,
    pub quantize: bool,
    pub quantize_ms: u64,
    pub accuracy: u64,
    pub live: bool,
    pub file_stamp: Option<(u64, Option<SystemTime>)>,
}
impl Snapshot {
    pub fn stamp(path: &std::path::Path) -> Option<(u64, Option<SystemTime>)> {
        std::fs::metadata(path)
            .ok()
            .map(|m| (m.len(), m.modified().ok()))
    }
    fn grid(&self) -> Duration {
        if self.quantize {
            Duration::from_millis(self.quantize_ms)
        } else {
            Duration::ZERO
        }
    }
}

pub fn validate(config: &Config) -> Result<(), String> {
    if !config.routing {
        return Err("Enable Range Instrument Support first".into());
    }
    let mut types = HashSet::new();
    for slot in config.slots.iter().filter(|s| s.enabled) {
        if !types.insert(slot.inst_type) {
            return Err(format!(
                "Choose unique instrument types: {} is enabled more than once",
                slot.inst_type.label()
            ));
        }
    }
    if types.is_empty() {
        return Err("Enable at least one instrument slot".into());
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub total: u64,
    pub original: u64,
    pub folded: u64,
}
impl Counts {
    pub fn played(self) -> u64 {
        self.original + self.folded
    }
    pub fn skipped(self) -> u64 {
        self.total - self.played()
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub notes: Counts,
    pub slots: [Counts; 4],
    pub cutoffs: u64,
    pub steps: u64,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
struct Layout([(u8, u8, i8); 4]);
impl Layout {
    fn from_slots(slots: &[InstrumentSlot; 4]) -> Self {
        Self(slots.map(|s| (s.min_note, s.max_note, s.transposition)))
    }
    fn apply(self, config: &Config) -> Config {
        let mut result = config.clone();
        for (slot, (min, max, transpose)) in result.slots.iter_mut().zip(self.0) {
            if slot.enabled {
                slot.min_note = min;
                slot.max_note = max;
                slot.transposition = transpose;
            }
        }
        result
    }
    fn magnitude(self, config: &Config) -> i32 {
        self.0
            .iter()
            .zip(config.slots)
            .filter(|(_, s)| s.enabled)
            .map(|(v, _)| (v.2 as i32).abs())
            .sum()
    }
}

pub struct DetectionResult {
    pub slots: [InstrumentSlot; 4],
    pub before: Stats,
    pub after: Stats,
    pub simulations: usize,
}

pub struct Job {
    pub snapshot: Snapshot,
    pub cancel: Arc<AtomicBool>,
    pub phase: Arc<AtomicUsize>,
    pub completed: Arc<AtomicUsize>,
    receiver: mpsc::Receiver<Result<DetectionResult, String>>,
    worker: Option<JoinHandle<()>>,
}
impl Job {
    pub fn start(snapshot: Snapshot) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let phase = Arc::new(AtomicUsize::new(0));
        let completed = Arc::new(AtomicUsize::new(0));
        let (tx, receiver) = mpsc::channel();
        let (input, stop, stage, count) = (
            snapshot.clone(),
            cancel.clone(),
            phase.clone(),
            completed.clone(),
        );
        let worker = thread::spawn(move || {
            let _ = tx.send(detect(&input, &stop, &stage, &count));
        });
        Self {
            snapshot,
            cancel,
            phase,
            completed,
            receiver,
            worker: Some(worker),
        }
    }
    pub fn is_current(&self, current: Option<&Snapshot>) -> bool {
        !self.cancel.load(Ordering::Relaxed) && Some(&self.snapshot) == current
    }
    pub fn poll(&mut self) -> Option<Result<DetectionResult, String>> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(Err("Auto-detect worker stopped unexpectedly".into()))
            }
        }
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn check(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Detection cancelled".into())
    } else {
        Ok(())
    }
}
fn note_on(bytes: &[u8]) -> bool {
    bytes.len() >= 3 && bytes[0] & 0xf0 == 0x90 && bytes[2] > 0
}
fn matching_slots(cfg: &Config, channel: u8, program: u8) -> [bool; 4] {
    cfg.slots.map(|s| {
        s.enabled
            && if s.inst_type == InstrumentType::Other {
                channel != 9
            } else {
                match_instrument(channel, program, s.inst_type)
            }
    })
}

struct SilentOutput<'a> {
    now: Instant,
    cancel: &'a AtomicBool,
}
impl Output for SilentOutput<'_> {
    fn valid(&self) -> bool {
        !self.cancel.load(Ordering::Relaxed)
    }
    fn key(&mut self, _: KeyCode, _: bool) -> Result<(), Cancelled> {
        if self.valid() { Ok(()) } else { Err(Cancelled) }
    }
    fn release(&mut self, _: KeyCode) {}
    fn wait(&mut self, duration: Duration) -> Result<(), Cancelled> {
        self.now += duration;
        if self.valid() { Ok(()) } else { Err(Cancelled) }
    }
}

fn simulate(
    song: &Song,
    cfg: &Config,
    grid: Duration,
    cancel: &AtomicBool,
) -> Result<Stats, String> {
    let origin = Instant::now();
    let mut io = SilentOutput {
        now: origin,
        cancel,
    };
    let mut pipeline = Pipeline::new();
    let mut stats = Stats::default();
    let mut programs = [0u8; 16];
    let mut arrivals = 0;
    // Quantization preserves stable event order; output waits advance only the consumer clock.
    let mut previous_due = origin;
    for (at, bytes) in &song.events {
        check(cancel)?;
        let due = deadline(origin + *at, origin, grid, origin).max(previous_due);
        previous_due = due;
        while let Some(return_at) = pipeline.next_return() {
            if return_at >= due || io.now >= due {
                break;
            }
            // Incoming notes reset idle return even while waiting for their quantized deadline.
            observe_arrivals(song, &mut arrivals, return_at, origin, cfg, &mut pipeline);
            let Some(next) = pipeline.next_return() else {
                break;
            };
            if next > return_at {
                continue;
            }
            io.now = io.now.max(return_at);
            pipeline.outcome = Default::default();
            let now = io.now;
            pipeline
                .return_tick(cfg, &mut io, now)
                .map_err(|_| "Detection cancelled")?;
            stats.steps += pipeline.outcome.transpose_steps;
        }
        io.now = io.now.max(due);
        observe_arrivals(song, &mut arrivals, io.now, origin, cfg, &mut pipeline);
        let channel = (bytes[0] & 15) as usize;
        if bytes[0] & 0xf0 == 0xc0 {
            programs[channel] = bytes[1];
        }
        let is_note = note_on(bytes);
        if is_note {
            stats.notes.total += 1;
            let matches = matching_slots(cfg, channel as u8, programs[channel]);
            let destinations = route(cfg, channel as u8, programs[channel], bytes[1]);
            let typed_destination = destinations.iter().any(|d| {
                d.slot
                    .is_some_and(|i| cfg.slots[i].inst_type != InstrumentType::Other)
            });
            for (i, matched) in matches.into_iter().enumerate() {
                if matched
                    && (cfg.slots[i].inst_type != InstrumentType::Other || !typed_destination)
                {
                    stats.slots[i].total += 1;
                }
            }
        }
        let event = MidiEvent::at(bytes.clone(), 0, origin + *at);
        let now = io.now;
        pipeline
            .process_at(&event, cfg, &mut io, now)
            .map_err(|_| "Detection cancelled")?;
        let outcome = pipeline.outcome;
        stats.cutoffs += outcome.cutoffs;
        stats.steps += outcome.transpose_steps;
        if is_note {
            let played = outcome.triggered.into_iter().any(|p| p);
            let original = outcome.unfolded.into_iter().any(|p| p);
            if original {
                stats.notes.original += 1;
            } else if played {
                stats.notes.folded += 1;
            }
            for i in 0..4 {
                if outcome.unfolded[i] {
                    stats.slots[i].original += 1;
                } else if outcome.triggered[i] {
                    stats.slots[i].folded += 1;
                }
            }
        }
    }
    // Include the silent tail, which can contain transpose-return work.
    while let Some(at) = pipeline.next_return() {
        if at > origin + song.total {
            break;
        }
        check(cancel)?;
        io.now = io.now.max(at);
        pipeline.outcome = Default::default();
        let now = io.now;
        pipeline
            .return_tick(cfg, &mut io, now)
            .map_err(|_| "Detection cancelled")?;
        stats.steps += pipeline.outcome.transpose_steps;
    }
    Ok(stats)
}
fn observe_arrivals(
    song: &Song,
    cursor: &mut usize,
    through: Instant,
    origin: Instant,
    cfg: &Config,
    pipeline: &mut Pipeline,
) {
    while let Some((at, bytes)) = song.events.get(*cursor) {
        if origin + *at > through {
            break;
        }
        if note_on(bytes) {
            pipeline.note_received(cfg.solver, origin + *at);
        }
        *cursor += 1;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Estimated {
    score: u64,
    original: u64,
    magnitude: i32,
    layout: Layout,
}
impl Ord for Estimated {
    fn cmp(&self, other: &Self) -> Cmp {
        self.score
            .cmp(&other.score)
            .then(self.original.cmp(&other.original))
            .then(other.magnitude.cmp(&self.magnitude))
            .then(other.layout.cmp(&self.layout))
    }
}
impl PartialOrd for Estimated {
    fn partial_cmp(&self, other: &Self) -> Option<Cmp> {
        Some(self.cmp(other))
    }
}
#[derive(Clone, Copy)]
struct Interval {
    score: u64,
    original: u64,
    transpose: i8,
}
type Tables = Vec<Vec<Vec<Interval>>>;
fn interval_index(min: usize, max: usize) -> usize {
    min * 89 + max
}

fn tables(
    song: &Song,
    cfg: &Config,
    enabled: &[usize],
    cancel: &AtomicBool,
) -> Result<Tables, String> {
    let mut histograms = [[0u64; 128]; 4];
    let mut programs = [0u8; 16];
    for (_, bytes) in &song.events {
        check(cancel)?;
        let channel = (bytes[0] & 15) as usize;
        if bytes[0] & 0xf0 == 0xc0 {
            programs[channel] = bytes[1];
        }
        if note_on(bytes) {
            for (i, matched) in matching_slots(cfg, channel as u8, programs[channel])
                .into_iter()
                .enumerate()
            {
                if matched {
                    histograms[i][bytes[1] as usize] += 1;
                }
            }
        }
    }
    if !enabled
        .iter()
        .any(|&i| histograms[i].iter().sum::<u64>() > 0)
    {
        return Err(
            "The selected MIDI contains no notes relevant to the enabled instruments".into(),
        );
    }
    let mut result = Vec::new();
    for &slot in enabled {
        let mut intervals = vec![Vec::new(); 89 * 89];
        for transpose in -24i8..=24 {
            check(cancel)?;
            let mut prefix = [0u64; 89];
            let mut classes = [0u64; 12];
            for (note, &count) in histograms[slot].iter().enumerate() {
                let physical = note as i32 - 20 - transpose as i32;
                classes[physical.rem_euclid(12) as usize] += count;
                if (1..=88).contains(&physical) {
                    prefix[physical as usize] += count;
                }
            }
            for key in 1..=88 {
                prefix[key] += prefix[key - 1];
            }
            for min in 1..=88 {
                let mut folded = 0;
                for max in min..=88 {
                    if max - min < 12 {
                        folded += classes[max % 12];
                    }
                    let original = prefix[max] - prefix[min - 1];
                    intervals[interval_index(min, max)].push(Interval {
                        score: if cfg.fold_octaves { folded } else { original },
                        original,
                        transpose,
                    });
                }
            }
        }
        for interval in &mut intervals {
            interval.sort_by_key(|v| {
                (
                    std::cmp::Reverse(v.score),
                    std::cmp::Reverse(v.original),
                    (v.transpose as i32).abs(),
                    v.transpose,
                )
            });
            interval.truncate(KEEP);
        }
        result.push(intervals);
    }
    Ok(result)
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Entry {
    estimate: Estimated,
    slot: usize,
    cut: usize,
    previous: usize,
    alternative: usize,
}
// Each transition is a sorted Cartesian product. Heap merging avoids materializing all pairs.
fn entry(
    dp: &[Vec<Vec<Estimated>>],
    table: &Tables,
    enabled: &[usize],
    mask: usize,
    end: usize,
    mut row: Entry,
) -> Option<Entry> {
    let previous = dp[mask ^ (1 << row.slot)][row.cut].get(row.previous)?;
    let interval = table[row.slot][interval_index(row.cut + 1, end)].get(row.alternative)?;
    let mut layout = previous.layout;
    layout.0[enabled[row.slot]] = ((row.cut + 1) as u8, end as u8, interval.transpose);
    row.estimate = Estimated {
        score: previous.score + interval.score,
        original: previous.original + interval.original,
        magnitude: previous.magnitude + (interval.transpose as i32).abs(),
        layout,
    };
    Some(row)
}
fn candidates(song: &Song, cfg: &Config, cancel: &AtomicBool) -> Result<Vec<Layout>, String> {
    let enabled: Vec<_> = cfg
        .slots
        .iter()
        .enumerate()
        .filter(|(_, s)| s.enabled)
        .map(|(i, _)| i)
        .collect();
    let table = tables(song, cfg, &enabled, cancel)?;
    let size = 1 << enabled.len();
    let base = Estimated {
        score: 0,
        original: 0,
        magnitude: 0,
        layout: Layout::from_slots(&cfg.slots),
    };
    let mut dp = vec![vec![Vec::new(); 89]; size];
    dp[0][0].push(base);
    for mask in 1usize..size {
        for end in mask.count_ones() as usize..=88 {
            check(cancel)?;
            let mut heap = BinaryHeap::new();
            for slot in 0..enabled.len() {
                if mask & (1 << slot) == 0 {
                    continue;
                }
                for cut in 0..end {
                    let row = Entry {
                        estimate: base,
                        slot,
                        cut,
                        previous: 0,
                        alternative: 0,
                    };
                    if let Some(row) = entry(&dp, &table, &enabled, mask, end, row) {
                        heap.push(row);
                    }
                }
            }
            let mut best = Vec::new();
            while best.len() < KEEP {
                let Some(row) = heap.pop() else {
                    break;
                };
                best.push(row.estimate);
                let next = Entry {
                    alternative: row.alternative + 1,
                    ..row
                };
                if let Some(next) = entry(&dp, &table, &enabled, mask, end, next) {
                    heap.push(next);
                }
                if row.alternative == 0 {
                    let next = Entry {
                        previous: row.previous + 1,
                        ..row
                    };
                    if let Some(next) = entry(&dp, &table, &enabled, mask, end, next) {
                        heap.push(next);
                    }
                }
            }
            dp[mask][end] = best;
        }
    }
    Ok(dp[size - 1][88].iter().map(|e| e.layout).collect())
}

fn compare(a: &(Layout, Stats), b: &(Layout, Stats), cfg: &Config) -> Cmp {
    a.1.notes
        .played()
        .cmp(&b.1.notes.played())
        .then(a.1.notes.original.cmp(&b.1.notes.original))
        .then(b.1.cutoffs.cmp(&a.1.cutoffs))
        .then(b.1.steps.cmp(&a.1.steps))
        .then(b.0.magnitude(cfg).cmp(&a.0.magnitude(cfg)))
        .then(b.0.cmp(&a.0))
}
fn neighbors(layout: Layout, cfg: &Config) -> Vec<Layout> {
    let mut order: Vec<_> = (0..4).filter(|&i| cfg.slots[i].enabled).collect();
    let mut result = Vec::new();
    for &i in &order {
        for transpose in -24..=24 {
            if transpose != layout.0[i].2 {
                let mut next = layout;
                next.0[i].2 = transpose;
                result.push(next);
            }
        }
    }
    order.sort_by_key(|&i| layout.0[i].0);
    for pair in order.windows(2) {
        let (left, right) = (pair[0], pair[1]);
        for boundary in layout.0[left].0..layout.0[right].1 {
            if boundary == layout.0[left].1 {
                continue;
            }
            let mut next = layout;
            next.0[left].1 = boundary;
            next.0[right].0 = boundary + 1;
            result.push(next);
        }
    }
    result
}
fn evaluate(
    layout: Layout,
    snapshot: &Snapshot,
    song: &Song,
    cancel: &AtomicBool,
    completed: &AtomicUsize,
    cache: &mut HashMap<Layout, Stats>,
) -> Result<Stats, String> {
    if let Some(stats) = cache.get(&layout) {
        return Ok(stats.clone());
    }
    let stats = simulate(
        song,
        &layout.apply(&snapshot.config),
        snapshot.grid(),
        cancel,
    )?;
    cache.insert(layout, stats.clone());
    completed.fetch_add(1, Ordering::Relaxed);
    Ok(stats)
}
fn detect(
    snapshot: &Snapshot,
    cancel: &AtomicBool,
    phase: &AtomicUsize,
    completed: &AtomicUsize,
) -> Result<DetectionResult, String> {
    validate(&snapshot.config)?;
    let song = analysis_events_edited(
        &snapshot.path,
        &snapshot.mutes,
        snapshot.content_hash.as_deref(),
        &|| cancel.load(Ordering::Relaxed),
    )?;
    if !song
        .events
        .iter()
        .any(|(_, bytes)| bytes[0] & 0xf0 == 0x90 && bytes[2] > 0)
    {
        return Err("No unmuted notes to analyze".into());
    }
    phase.store(1, Ordering::Relaxed);
    let layouts = candidates(&song, &snapshot.config, cancel)?;
    phase.store(2, Ordering::Relaxed);
    let mut cache = HashMap::new();
    let before = evaluate(
        Layout::from_slots(&snapshot.config.slots),
        snapshot,
        &song,
        cancel,
        completed,
        &mut cache,
    )?;
    let mut ranked = Vec::new();
    for layout in layouts {
        let stats = evaluate(layout, snapshot, &song, cancel, completed, &mut cache)?;
        ranked.push((layout, stats));
    }
    ranked.sort_by(|a, b| compare(b, a, &snapshot.config));
    let mut seeds: Vec<_> = ranked.iter().take(4).cloned().collect();
    phase.store(3, Ordering::Relaxed);
    'passes: for _ in 0..3 {
        let mut changed = false;
        for seed in &mut seeds {
            let mut best = seed.clone();
            for next in neighbors(seed.0, &snapshot.config) {
                check(cancel)?;
                if completed.load(Ordering::Relaxed) >= LIMIT {
                    break 'passes;
                }
                let stats = evaluate(next, snapshot, &song, cancel, completed, &mut cache)?;
                let next = (next, stats);
                if compare(&next, &best, &snapshot.config).is_gt() {
                    best = next.clone();
                }
                // Retain improvements even if the simulation budget ends mid-pass.
                ranked.push(next);
            }
            changed |= best.0 != seed.0;
            *seed = best;
        }
        if !changed {
            break;
        }
    }
    check(cancel)?;
    let (layout, after) = ranked
        .into_iter()
        .max_by(|a, b| compare(a, b, &snapshot.config))
        .ok_or("No layouts found")?;
    Ok(DetectionResult {
        slots: layout.apply(&snapshot.config).slots,
        before,
        after,
        simulations: completed.load(Ordering::Relaxed),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solver::MappingGroups;

    fn config() -> Config {
        Config {
            groups: MappingGroups {
                low: true,
                middle: true,
                high: true,
            },
            solver: true,
            max_jump: 48,
            range: 24,
            auto_octave: false,
            experimental: true,
            hold_ctrl: true,
            sustain: true,
            velocity: false,
            routing: true,
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
    fn song(events: &[(u64, &[u8])]) -> Song {
        Song {
            events: events
                .iter()
                .map(|(ms, b)| (Duration::from_millis(*ms), b.to_vec()))
                .collect(),
            total: Duration::from_millis(events.last().map_or(0, |e| e.0) + 2000),
        }
    }
    fn snapshot(config: Config) -> Snapshot {
        Snapshot {
            content_hash: None,
            mutes: Mutes::default(),
            config,
            path: PathBuf::from("unused.mid"),
            quantize: false,
            quantize_ms: 100,
            accuracy: 100,
            live: false,
            file_stamp: None,
        }
    }

    #[test]
    fn validation_requires_enabled_unique_types_and_routing() {
        let mut cfg = config();
        assert!(validate(&cfg).unwrap_err().contains("at least one"));
        cfg.slots[0].enabled = true;
        assert!(validate(&cfg).is_ok());
        cfg.slots[1].enabled = true;
        assert!(validate(&cfg).unwrap_err().contains("unique"));
        cfg.slots[1].inst_type = InstrumentType::Bass;
        assert!(validate(&cfg).is_ok());
        cfg.routing = false;
        assert!(validate(&cfg).is_err());
    }

    #[test]
    fn simulation_counts_program_changes_percussion_folding_and_velocity_zero() {
        let mut cfg = config();
        cfg.slots[0].enabled = true;
        cfg.slots[0].min_note = 40;
        cfg.slots[0].max_note = 52;
        cfg.slots[1].enabled = true;
        cfg.slots[1].inst_type = InstrumentType::Bass;
        cfg.slots[2].enabled = true;
        cfg.slots[2].inst_type = InstrumentType::Drum;
        let song = song(&[
            (0, &[0x90, 60, 100]),
            (20, &[0x90, 60, 0]),
            (40, &[0x90, 84, 100]),
            (60, &[0xc0, 32]),
            (80, &[0x90, 36, 100]),
            (100, &[0x99, 60, 100]),
            (120, &[0x90, 36, 100]),
        ]);
        let stop = AtomicBool::new(false);
        let stats = simulate(&song, &cfg, Duration::ZERO, &stop).unwrap();
        assert_eq!(stats.notes.total, 5);
        assert_eq!(stats.notes.played(), 4);
        assert_eq!(
            stats.slots[0],
            Counts {
                total: 2,
                original: 1,
                folded: 0
            }
        );
        assert_eq!(stats.slots[1].total, 2);
        assert_eq!(stats.slots[2].played(), 1);
        cfg.fold_octaves = true;
        let folded = simulate(&song, &cfg, Duration::ZERO, &stop).unwrap();
        assert_eq!(folded.notes.played(), 5);
        assert_eq!(folded.notes.folded, 1);
        assert_eq!(folded.slots[0].folded, 1);
        // Same score on repeat, independent of wall clock and hash-map iteration.
        assert_eq!(
            folded,
            simulate(&song, &cfg, Duration::ZERO, &stop).unwrap()
        );
    }

    #[test]
    fn simulation_honors_other_fallback_solver_skips_and_silent_tail() {
        let mut cfg = config();
        cfg.slots[0].enabled = true;
        cfg.slots[0].min_note = 40;
        cfg.slots[0].max_note = 40; // MIDI 60
        cfg.max_jump = 3;
        let music = song(&[(0, &[0x90, 60, 100]), (20, &[0x90, 90, 100])]);
        let stop = AtomicBool::new(false);
        assert_eq!(
            simulate(&music, &cfg, Duration::ZERO, &stop)
                .unwrap()
                .notes
                .played(),
            1
        );
        cfg.slots[1].enabled = true;
        cfg.slots[1].inst_type = InstrumentType::Other;
        let stats = simulate(&music, &cfg, Duration::ZERO, &stop).unwrap();
        assert_eq!(stats.notes.played(), 2);
        assert_eq!(stats.slots[1].total, 1);
        assert_eq!(stats.slots[1].played(), 1);
        cfg.slots[1].enabled = false;
        cfg.slots[0].min_note = 41; // MIDI 61, so playing 60 needs -1 global transpose
        cfg.slots[0].max_note = 41;
        cfg.slots[0].transposition = -1;
        let original = song(&[(0, &[0x90, 60, 100])]);
        let stats = simulate(&original, &cfg, Duration::ZERO, &stop).unwrap();
        assert_eq!(stats.notes.played(), 1);
        assert_eq!(stats.steps, 0); // compensated target is already 61
        // Use a white-key mapping via transpose, then count the idle return in the tail.
        cfg.slots[0].transposition = 0;
        cfg.slots[0].min_note = 40;
        cfg.slots[0].max_note = 42;
        let sharp = song(&[(0, &[0x90, 61, 100])]);
        let stats = simulate(&sharp, &cfg, Duration::ZERO, &stop).unwrap();
        // Solver may use the direct black-key mapping at zero, which needs no return.
        assert_eq!(stats.notes.played(), 1);
    }

    #[test]
    fn simulation_reports_key_theft_and_output_delays_without_sleeping() {
        let mut cfg = config();
        cfg.slots[0].enabled = true;
        cfg.slots[0].min_note = 40;
        cfg.slots[0].max_note = 52;
        // One physical QWERTY key represents both of these octave-separated pitches.
        let mappings = crate::solver::get_available_mappings();
        let (a, b) = mappings
            .iter()
            .find_map(|a| {
                mappings
                    .iter()
                    .find(|b| {
                        a.midi_note != b.midi_note
                            && a.key_code == b.key_code
                            && a.midi_note >= 36
                            && b.midi_note <= 96
                    })
                    .map(|b| (a.midi_note, b.midi_note))
            })
            .unwrap();
        cfg.slots[0].min_note = 1;
        cfg.slots[0].max_note = 88;
        cfg.solver = false;
        cfg.experimental = false;
        cfg.hold_ctrl = true;
        let music = song(&[(0, &[0x90, a, 100]), (0, &[0x90, b, 100])]);
        let stats = simulate(&music, &cfg, Duration::ZERO, &AtomicBool::new(false)).unwrap();
        assert_eq!(stats.notes.played(), 2);
        assert_eq!(stats.cutoffs, 1);
    }

    #[test]
    fn interval_estimates_match_brute_force_octave_fitting() {
        let mut cfg = config();
        cfg.slots[0].enabled = true;
        cfg.fold_octaves = true;
        let music = song(&[
            (0, &[0x90, 0, 100]),
            (0, &[0x90, 60, 100]),
            (0, &[0x90, 60, 100]),
            (0, &[0x90, 127, 100]),
        ]);
        let tables = tables(&music, &cfg, &[0], &AtomicBool::new(false)).unwrap();
        for (min, max) in [(1, 1), (1, 88), (40, 42), (40, 51), (88, 88)] {
            for value in &tables[0][interval_index(min, max)] {
                let score = [0, 60, 60, 127]
                    .into_iter()
                    .filter(|note| {
                        crate::pipeline::fit_pitch(
                            note - 20 - value.transpose as i32,
                            min as u8,
                            max as u8,
                            true,
                        )
                        .is_some()
                    })
                    .count();
                assert_eq!(value.score, score as u64);
            }
        }
    }

    #[test]
    fn dp_matches_exhaustive_single_slot_ranking_and_can_reverse_slot_order() {
        let stop = AtomicBool::new(false);
        let mut cfg = config();
        cfg.slots[0].enabled = true;
        let music = song(&[
            (0, &[0x90, 0, 100]),
            (0, &[0x90, 60, 100]),
            (0, &[0x90, 127, 100]),
        ]);
        let choices = candidates(&music, &cfg, &stop).unwrap();
        let mut exhaustive: Vec<_> = (-24i8..=24)
            .map(|t| {
                let mut layout = Layout::from_slots(&cfg.slots);
                layout.0[0] = (1, 88, t);
                let covered = [0i32, 60, 127]
                    .into_iter()
                    .filter(|note| (1..=88).contains(&(note - 20 - t as i32)))
                    .count();
                Estimated {
                    score: covered as u64,
                    original: covered as u64,
                    magnitude: (t as i32).abs(),
                    layout,
                }
            })
            .collect();
        exhaustive.sort_by(|a, b| b.cmp(a));
        assert_eq!(
            choices,
            exhaustive
                .iter()
                .take(KEEP)
                .map(|e| e.layout)
                .collect::<Vec<_>>()
        );
        cfg.slots[1].enabled = true;
        cfg.slots[1].inst_type = InstrumentType::Bass;
        let music = song(&[
            (0, &[0x90, 100, 100]),
            (0, &[0xc1, 32]),
            (0, &[0x91, 30, 100]),
        ]);
        let choices = candidates(&music, &cfg, &stop).unwrap();
        let best = choices[0];
        assert_eq!(best.0[1].0, 1);
        assert_eq!(best.0[0].1, 88);
        assert_eq!(best.0[1].1 + 1, best.0[0].0);
        assert_eq!((best.0[0].2, best.0[1].2), (0, 0));
        assert_eq!(choices, candidates(&music, &cfg, &stop).unwrap());
    }

    #[test]
    fn empty_unmatched_and_cancelled_searches_return_errors() {
        let mut cfg = config();
        cfg.slots[0].enabled = true;
        let stop = AtomicBool::new(false);
        assert!(candidates(&song(&[]), &cfg, &stop).is_err());
        cfg.slots[0].inst_type = InstrumentType::Bass;
        assert!(candidates(&song(&[(0, &[0x90, 60, 100])]), &cfg, &stop).is_err());
        stop.store(true, Ordering::Relaxed);
        assert!(simulate(&song(&[(0, &[0x90, 60, 100])]), &cfg, Duration::ZERO, &stop).is_err());
        assert!(candidates(&song(&[]), &cfg, &stop).is_err());
    }

    #[test]
    fn snapshot_guards_reject_edits_selection_changes_and_cancelled_results() {
        let mut cfg = config();
        cfg.slots[0].enabled = true;
        let input = snapshot(cfg);
        let job = Job::start(input.clone()); // nonexistent file; never touches hardware
        assert!(job.is_current(Some(&input)));
        let mut changed = input.clone();
        changed.config.slots[0].min_note += 1;
        assert!(!job.is_current(Some(&changed)));
        changed = input.clone();
        changed.config.fold_octaves = true;
        assert!(!job.is_current(Some(&changed)));
        changed = input.clone();
        changed.quantize = true;
        assert!(!job.is_current(Some(&changed)));
        changed = input.clone();
        changed.path = PathBuf::from("another.mid");
        assert!(!job.is_current(Some(&changed)));
        changed = input.clone();
        changed
            .mutes
            .muted
            .insert(crate::midi_player::InstrumentId {
                track: 0,
                program: 0,
                percussion: false,
            });
        assert!(!job.is_current(Some(&changed)));
        changed = input.clone();
        changed.content_hash = Some("changed contents".into());
        assert!(!job.is_current(Some(&changed)));
        assert!(!job.is_current(None));
        job.cancel.store(true, Ordering::Relaxed);
        assert!(!job.is_current(Some(&input)));
    }
    #[test]
    fn full_detection_reads_midi_and_preserves_slots_with_bounded_search() {
        use midly::{
            Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind,
        };
        let path =
            std::env::temp_dir().join(format!("miditoroblox-detect-{}.mid", std::process::id()));
        let midi = |channel: u8, message| TrackEvent {
            delta: 0.into(),
            kind: TrackEventKind::Midi {
                channel: channel.into(),
                message,
            },
        };
        let smf = Smf {
            header: Header::new(Format::SingleTrack, Timing::Metrical(480.into())),
            tracks: vec![vec![
                midi(
                    0,
                    MidiMessage::NoteOn {
                        key: 100.into(),
                        vel: 100.into(),
                    },
                ),
                midi(1, MidiMessage::ProgramChange { program: 32.into() }),
                midi(
                    1,
                    MidiMessage::NoteOn {
                        key: 30.into(),
                        vel: 100.into(),
                    },
                ),
                TrackEvent {
                    delta: 960.into(),
                    kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
                },
            ]],
        };
        let mut bytes = Vec::new();
        smf.write_std(&mut bytes).unwrap();
        std::fs::write(&path, bytes).unwrap();
        let mut cfg = config();
        cfg.slots[0].enabled = true;
        cfg.slots[0].min_note = 40;
        cfg.slots[0].max_note = 52;
        cfg.slots[1].enabled = true;
        cfg.slots[1].inst_type = InstrumentType::Bass;
        cfg.slots[1].min_note = 40;
        cfg.slots[1].max_note = 52;
        let mut input = snapshot(cfg.clone());
        input.path = path.clone();
        let stop = AtomicBool::new(false);
        let phase = AtomicUsize::new(0);
        let completed = AtomicUsize::new(0);
        let result = detect(&input, &stop, &phase, &completed).unwrap();
        assert_eq!(result.before.notes.played(), 0);
        assert_eq!(result.after.notes.played(), 2);
        assert!(result.simulations > KEEP && result.simulations <= LIMIT);
        assert_eq!(result.slots[1].min_note, 1);
        assert_eq!(result.slots[0].max_note, 88);
        assert_eq!(result.slots[1].max_note + 1, result.slots[0].min_note);
        for (before, after) in cfg.slots.into_iter().zip(result.slots) {
            assert_eq!(before.enabled, after.enabled);
            assert_eq!(before.inst_type, after.inst_type);
            if !before.enabled {
                assert_eq!(before, after);
            }
        }
        assert_eq!(input.config, cfg);
        let parsed = analysis_events(&path, &|| false).unwrap();
        assert_eq!(parsed.total, Duration::from_secs(1));
        assert_eq!(parsed.events[0].0, Duration::ZERO);
        assert!(analysis_events(&path, &|| true).is_err());
        input.mutes.muted.insert(crate::midi_player::InstrumentId {
            track: 0,
            program: 32,
            percussion: false,
        });
        let edited_song = analysis_events_edited(&path, &input.mutes, None, &|| false).unwrap();
        let mut reference = analysis_events(&path, &|| false).unwrap();
        reference
            .events
            .retain(|(_, b)| !(b[0] == 0x91 && b[1] == 30));
        assert_eq!(edited_song.events, reference.events);
        assert_eq!(
            simulate(&edited_song, &cfg, Duration::ZERO, &stop).unwrap(),
            simulate(&reference, &cfg, Duration::ZERO, &stop).unwrap()
        );
        assert_eq!(
            candidates(&edited_song, &cfg, &stop).unwrap(),
            candidates(&reference, &cfg, &stop).unwrap()
        );
        let edited_result = detect(&input, &stop, &phase, &AtomicUsize::new(0)).unwrap();
        assert_eq!(edited_result.after.notes.played(), 1);
        input.mutes.muted.insert(crate::midi_player::InstrumentId {
            track: 0,
            program: 0,
            percussion: false,
        });
        assert_eq!(
            detect(&input, &stop, &phase, &AtomicUsize::new(0))
                .err()
                .unwrap(),
            "No unmuted notes to analyze"
        );
        assert_eq!(input.config, cfg);
        std::fs::write(&path, b"not MIDI").unwrap();
        assert!(detect(&input, &stop, &phase, &AtomicUsize::new(0)).is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn four_slots_search_all_orders_and_preserve_partition_during_refinement() {
        let stop = AtomicBool::new(false);
        let mut cfg = config();
        for (slot, inst_type) in cfg.slots.iter_mut().zip([
            InstrumentType::Synth,
            InstrumentType::Piano,
            InstrumentType::Bass,
            InstrumentType::Strings,
        ]) {
            slot.enabled = true;
            slot.inst_type = inst_type;
        }
        let music = song(&[
            (0, &[0xc0, 80]),
            (0, &[0x90, 110, 100]),
            (0, &[0xc1, 0]),
            (0, &[0x91, 70, 100]),
            (0, &[0xc2, 32]),
            (0, &[0x92, 20, 100]),
            (0, &[0xc3, 40]),
            (0, &[0x93, 90, 100]),
        ]);
        let choices = candidates(&music, &cfg, &stop).unwrap();
        let mut order = [0, 1, 2, 3];
        order.sort_by_key(|&i| choices[0].0[i].0);
        assert_eq!(order, [2, 1, 3, 0]);
        assert_eq!(choices.len(), KEEP);
        assert_eq!(choices.iter().collect::<HashSet<_>>().len(), KEEP);
        for layout in choices.iter().copied().chain(neighbors(choices[0], &cfg)) {
            let mut ranges = layout.0;
            ranges.sort_by_key(|v| v.0);
            assert_eq!(ranges[0].0, 1);
            assert_eq!(ranges[3].1, 88);
            for pair in ranges.windows(2) {
                assert_eq!(pair[0].1 + 1, pair[1].0);
            }
            assert!(
                ranges
                    .iter()
                    .all(|v| v.0 <= v.1 && (-24..=24).contains(&v.2))
            );
        }
    }

    #[test]
    fn scoring_priorities_prefer_coverage_then_originals_then_fewer_cutoffs_and_steps() {
        let mut cfg = config();
        cfg.slots[0].enabled = true;
        let layout = Layout::from_slots(&cfg.slots);
        let a = (
            layout,
            Stats {
                notes: Counts {
                    total: 2,
                    original: 1,
                    folded: 1,
                },
                ..Stats::default()
            },
        );
        let b = (
            layout,
            Stats {
                notes: Counts {
                    total: 2,
                    original: 1,
                    folded: 0,
                },
                ..Stats::default()
            },
        );
        assert!(compare(&a, &b, &cfg).is_gt());
        let mut best = a.clone();
        best.1.notes.original = 2;
        best.1.notes.folded = 0;
        assert!(compare(&best, &a, &cfg).is_gt());
        let mut worse = best.clone();
        worse.1.cutoffs = 1;
        assert!(compare(&best, &worse, &cfg).is_gt());
        worse = best.clone();
        worse.1.steps = 1;
        assert!(compare(&best, &worse, &cfg).is_gt());
        worse = best.clone();
        worse.0.0[0].2 = 1;
        assert!(compare(&best, &worse, &cfg).is_gt());
    }
    #[test]
    fn simulation_honors_quantization_jump_budget_sustain_and_idle_return() {
        let mut cfg = config();
        cfg.slots[0].enabled = true;
        cfg.slots[0].max_note = 40; // MIDI 21–60
        cfg.groups.low = false; // 35 needs a -1 transpose from MIDI 36
        cfg.max_jump = 3;
        let music = song(&[
            (1, &[0x90, 35, 100]),
            (110, &[0x90, 30, 100]), // needs -6; exceeds the current jump budget
            (150, &[0xb0, 64, 127]),
            (180, &[0x80, 35, 0]),
            (800, &[0xb0, 64, 0]),
            (1000, &[0x90, 35, 100]),
        ]);
        let result = simulate(
            &music,
            &cfg,
            Duration::from_millis(100),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(
            result.notes,
            Counts {
                total: 3,
                original: 2,
                folded: 0
            }
        );
        assert_eq!(result.steps, 4); // twice transpose down, twice return to zero
        assert_eq!(result.cutoffs, 0);
    }

    #[test]
    fn virtual_output_advances_waits_and_checks_cancellation() {
        let stop = AtomicBool::new(false);
        let start = Instant::now();
        let mut output = SilentOutput {
            now: start,
            cancel: &stop,
        };
        output.wait(Duration::from_secs(3600)).unwrap();
        assert_eq!(output.now.duration_since(start), Duration::from_secs(3600));
        output.key(KeyCode::KEY_1, true).unwrap();
        stop.store(true, Ordering::Relaxed);
        assert!(output.key(KeyCode::KEY_1, false).is_err());
        assert!(output.wait(Duration::ZERO).is_err());
        output.release(KeyCode::KEY_1);
    }
}
