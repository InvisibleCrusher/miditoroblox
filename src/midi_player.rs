use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

pub struct ScanDir {
    pub path: PathBuf,
    pub recursive: bool,
}

#[derive(PartialEq, Clone, Copy)]
pub enum PlaybackStatus {
    Stopped,
    Playing,
    Paused,
}

#[derive(Clone)]
enum EventKind {
    Midi(Vec<u8>),
    Tempo(u32),
}

#[derive(Clone)]
struct Event {
    abs_tick: u64,
    kind: EventKind,
}

enum Cmd {
    Load { events: Vec<Event>, tpb: u32 },
    Pause,
    Resume,
    Stop,
    Seek(u64),
}

pub struct MidiPlayer {
    pub scan_dirs: Vec<ScanDir>,
    pub midi_files: Vec<PathBuf>,
    pub search_query: String,
    pub selected_idx: Option<usize>,
    pub playing_idx: Option<usize>,
    status: Arc<Mutex<PlaybackStatus>>,
    position_ticks: Arc<AtomicU64>,
    total_ticks: Arc<AtomicU64>,
    cmd_tx: mpsc::Sender<Cmd>,
}

impl MidiPlayer {
    pub fn new(midi_tx: mpsc::Sender<Vec<u8>>) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let status = Arc::new(Mutex::new(PlaybackStatus::Stopped));
        let position_ticks = Arc::new(AtomicU64::new(0));
        let total_ticks = Arc::new(AtomicU64::new(0));

        let s = Arc::clone(&status);
        let p = Arc::clone(&position_ticks);
        thread::spawn(move || playback_thread(cmd_rx, midi_tx, s, p));

        Self {
            scan_dirs: Vec::new(),
            midi_files: Vec::new(),
            search_query: String::new(),
            selected_idx: None,
            playing_idx: None,
            status,
            position_ticks,
            total_ticks,
            cmd_tx,
        }
    }

    pub fn scan(&mut self) {
        self.midi_files.clear();
        for dir in &self.scan_dirs {
            collect_midi(&dir.path, dir.recursive, &mut self.midi_files);
        }
        self.midi_files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
        self.midi_files.dedup();
    }

    pub fn play(&mut self, idx: usize) {
        let Some(path) = self.midi_files.get(idx).cloned() else { return };
        let Some((events, tpb, total)) = parse_midi(&path) else { return };
        self.playing_idx = Some(idx);
        self.selected_idx = Some(idx);
        self.total_ticks.store(total, Ordering::Relaxed);
        let _ = self.cmd_tx.send(Cmd::Load { events, tpb });
    }

    pub fn pause(&self)           { let _ = self.cmd_tx.send(Cmd::Pause); }
    pub fn resume(&self)          { let _ = self.cmd_tx.send(Cmd::Resume); }
    pub fn seek(&self, tick: u64) { let _ = self.cmd_tx.send(Cmd::Seek(tick)); }

    pub fn stop(&mut self) {
        self.playing_idx = None;
        let _ = self.cmd_tx.send(Cmd::Stop);
    }

    pub fn status(&self) -> PlaybackStatus {
        *self.status.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn position(&self) -> u64 { self.position_ticks.load(Ordering::Relaxed) }
    pub fn total(&self)    -> u64 { self.total_ticks.load(Ordering::Relaxed) }

    pub fn filtered_files(&self) -> Vec<(usize, PathBuf)> {
        let q = self.search_query.to_lowercase();
        self.midi_files.iter().enumerate()
            .filter(|(_, p)| {
                if q.is_empty() { return true; }
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.to_lowercase().contains(&q))
                    .unwrap_or(false)
            })
            .map(|(i, p)| (i, p.clone()))
            .collect()
    }
}

fn collect_midi(dir: &PathBuf, recursive: bool, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() && recursive {
            collect_midi(&p, true, out);
        } else if p.is_file() {
            let is_midi = p.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("mid") || e.eq_ignore_ascii_case("midi"))
                .unwrap_or(false);
            if is_midi { out.push(p); }
        }
    }
}

fn parse_midi(path: &PathBuf) -> Option<(Vec<Event>, u32, u64)> {
    let data = std::fs::read(path).ok()?;
    let smf = midly::Smf::parse(&data).ok()?;

    let tpb = match smf.header.timing {
        midly::Timing::Metrical(t) => t.as_int() as u32,
        _ => return None,
    };

    let mut events: Vec<Event> = Vec::new();

    for track in &smf.tracks {
        let mut abs: u64 = 0;
        for ev in track {
            abs += ev.delta.as_int() as u64;
            match &ev.kind {
                midly::TrackEventKind::Midi { channel, message } => {
                    let ch = channel.as_int();
                    let bytes: Option<Vec<u8>> = match message {
                        midly::MidiMessage::NoteOn { key, vel } =>
                            Some(vec![0x90 | ch, key.as_int(), vel.as_int()]),
                        midly::MidiMessage::NoteOff { key, vel } =>
                            Some(vec![0x80 | ch, key.as_int(), vel.as_int()]),
                        midly::MidiMessage::Controller { controller, value } =>
                            Some(vec![0xB0 | ch, controller.as_int(), value.as_int()]),
                        midly::MidiMessage::ProgramChange { program } =>
                            Some(vec![0xC0 | ch, program.as_int()]),
                        _ => None,
                    };
                    if let Some(b) = bytes {
                        events.push(Event { abs_tick: abs, kind: EventKind::Midi(b) });
                    }
                }
                midly::TrackEventKind::Meta(midly::MetaMessage::Tempo(t)) => {
                    events.push(Event { abs_tick: abs, kind: EventKind::Tempo(t.as_int()) });
                }
                _ => {}
            }
        }
    }

    events.sort_by_key(|e| e.abs_tick);
    let total = events.last().map(|e| e.abs_tick).unwrap_or(0);
    Some((events, tpb, total))
}

fn release_held(held: &HashSet<(u8, u8)>, tx: &mpsc::Sender<Vec<u8>>) {
    for (ch, note) in held {
        let _ = tx.send(vec![0x80 | ch, *note, 0]);
    }
}

fn playback_thread(
    cmd_rx: mpsc::Receiver<Cmd>,
    midi_tx: mpsc::Sender<Vec<u8>>,
    status: Arc<Mutex<PlaybackStatus>>,
    position_ticks: Arc<AtomicU64>,
) {
    let mut events: Vec<Event> = Vec::new();
    let mut tpb: u32 = 480;
    let mut cursor: usize = 0;
    let mut tempo: u32 = 500_000;
    let mut held: HashSet<(u8, u8)> = HashSet::new();
    let mut playing = false;
    let mut next_at: Option<Instant> = None;

    loop {
        // Block when stopped/paused to avoid busy-wait
        let cmd = if playing {
            cmd_rx.try_recv().ok()
        } else {
            match cmd_rx.recv() {
                Ok(c) => Some(c),
                Err(_) => return,
            }
        };

        if let Some(cmd) = cmd {
            match cmd {
                Cmd::Load { events: new_ev, tpb: new_tpb } => {
                    release_held(&held, &midi_tx);
                    held.clear();
                    events = new_ev;
                    tpb = new_tpb;
                    cursor = 0;
                    tempo = 500_000;
                    playing = true;
                    next_at = Some(Instant::now());
                    position_ticks.store(0, Ordering::Relaxed);
                    if let Ok(mut s) = status.lock() { *s = PlaybackStatus::Playing; }
                }
                Cmd::Pause => {
                    release_held(&held, &midi_tx);
                    held.clear();
                    playing = false;
                    if let Ok(mut s) = status.lock() { *s = PlaybackStatus::Paused; }
                }
                Cmd::Resume => {
                    playing = true;
                    next_at = Some(Instant::now());
                    if let Ok(mut s) = status.lock() { *s = PlaybackStatus::Playing; }
                }
                Cmd::Stop => {
                    release_held(&held, &midi_tx);
                    held.clear();
                    events.clear();
                    cursor = 0;
                    playing = false;
                    next_at = None;
                    position_ticks.store(0, Ordering::Relaxed);
                    if let Ok(mut s) = status.lock() { *s = PlaybackStatus::Stopped; }
                }
                Cmd::Seek(tick) => {
                    release_held(&held, &midi_tx);
                    held.clear();
                    // Find the last tempo event before the seek point
                    tempo = 500_000;
                    for ev in &events {
                        if ev.abs_tick > tick { break; }
                        if let EventKind::Tempo(t) = &ev.kind { tempo = *t; }
                    }
                    cursor = events.partition_point(|e| e.abs_tick < tick);
                    position_ticks.store(tick, Ordering::Relaxed);
                    if playing { next_at = Some(Instant::now()); }
                }
            }
        }

        if !playing || events.is_empty() { continue; }

        if cursor >= events.len() {
            release_held(&held, &midi_tx);
            held.clear();
            events.clear();
            cursor = 0;
            playing = false;
            position_ticks.store(0, Ordering::Relaxed);
            if let Ok(mut s) = status.lock() { *s = PlaybackStatus::Stopped; }
            continue;
        }

        // Sleep in small chunks so commands are checked frequently
        if let Some(due) = next_at {
            let now = Instant::now();
            if now < due {
                thread::sleep((due - now).min(Duration::from_millis(5)));
                continue;
            }
        }

        let ev = &events[cursor];
        let ev_tick = ev.abs_tick;
        position_ticks.store(ev_tick, Ordering::Relaxed);

        match &ev.kind {
            EventKind::Midi(bytes) => {
                if bytes.len() >= 3 {
                    let s0 = bytes[0] & 0xF0;
                    let ch = bytes[0] & 0x0F;
                    let note = bytes[1];
                    let vel = bytes[2];
                    if s0 == 0x90 && vel > 0 {
                        held.insert((ch, note));
                    } else if s0 == 0x80 || (s0 == 0x90 && vel == 0) {
                        held.remove(&(ch, note));
                    }
                }
                let _ = midi_tx.send(bytes.clone());
            }
            EventKind::Tempo(t) => { tempo = *t; }
        }

        cursor += 1;

        if cursor < events.len() {
            let next_tick = events[cursor].abs_tick;
            let tick_diff = next_tick.saturating_sub(ev_tick);
            let us = if tpb > 0 { (tick_diff as u64 * tempo as u64) / tpb as u64 } else { 0 };
            next_at = Some(Instant::now() + Duration::from_micros(us));
        } else {
            next_at = None;
        }
    }
}
