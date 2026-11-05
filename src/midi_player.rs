use crate::midi_event::{MidiEvent, MidiSource};
use crate::settings::Store;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct InstrumentId {
    pub track: usize,
    pub program: u8,
    pub percussion: bool,
}
#[derive(Clone, Debug)]
pub struct InstrumentRow {
    pub id: InstrumentId,
    pub track_name: String,
    pub channels: BTreeSet<u8>,
    pub notes: usize,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Mutes {
    pub muted: BTreeSet<InstrumentId>,
}
impl Mutes {
    pub fn filename(hash: &str) -> String {
        format!("midi/{hash}.json")
    }
}
#[derive(Clone)]
pub struct Document {
    pub hash: String,
    pub instruments: Vec<InstrumentRow>,
    events: Vec<Event>,
    owners: Vec<Option<InstrumentId>>,
    tpb: u32,
}
impl Document {
    fn timeline(&self, mutes: &Mutes) -> Timeline {
        let events = self
            .events
            .iter()
            .zip(&self.owners)
            .filter(|(_, owner)| !owner.is_some_and(|id| mutes.muted.contains(&id)))
            .map(|(event, _)| event.clone())
            .collect();
        Timeline::new(events, self.tpb, Duration::ZERO)
    }
    fn load(path: &Path, cancelled: &impl Fn() -> bool) -> Result<Self, String> {
        let bytes =
            std::fs::read(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
        Self::parse(&bytes, cancelled).map_err(|e| format!("Cannot read {}: {e}", path.display()))
    }
}

pub const FOCUS_DELAY: Duration = Duration::from_millis(200);

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ScanDir {
    pub path: PathBuf,
    pub recursive: bool,
}

#[derive(PartialEq, Clone, Copy, Debug)]
pub enum PlaybackStatus {
    Stopped,
    Playing,
    Paused,
}

#[derive(Clone)]
enum EventKind {
    Midi(Vec<u8>),
    Tempo(u32),
    End,
}

#[derive(Clone)]
struct Event {
    abs_tick: u64,
    kind: EventKind,
}

enum Cmd {
    Load {
        timeline: Timeline,
        epoch: u64,
    },
    Pause(u64),
    Resume(u64),
    Stop(u64),
    Seek(Duration, u64),
    Replace {
        timeline: Timeline,
        epoch: u64,
        position: Duration,
    },
    #[cfg(test)]
    Barrier(mpsc::Sender<()>),
    Shutdown,
}

enum IoCmd {
    Scan(u64, Vec<ScanDir>),
    Load(u64, PathBuf),
    Inspect(u64, PathBuf),
    Shutdown,
}

enum Notice {
    Scanned(u64, Vec<PathBuf>, Vec<String>),
    Loaded(u64, PathBuf, Result<LoadedDocument, String>),
    Inspected(u64, PathBuf, Result<LoadedDocument, String>),
    Error(String),
}

type LoadedDocument = (Arc<Document>, Mutes, Option<String>);
fn load_document(
    path: &Path,
    store: &Mutex<Store>,
    cancelled: &impl Fn() -> bool,
) -> Result<LoadedDocument, String> {
    let document = Arc::new(Document::load(path, cancelled)?);
    let result = store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .read(&Mutes::filename(&document.hash), Mutes::default());
    let (mutes, warning) = match result {
        Ok(m) => (m, None),
        Err(e) => (Mutes::default(), Some(e)),
    };
    Ok((document, mutes, warning))
}

pub struct MidiPlayer {
    pub store: Arc<Mutex<Store>>,
    pub editor: Option<(PathBuf, Arc<Document>, Mutes)>,
    editor_path: Option<PathBuf>,
    editor_stamp: Option<(u64, Option<SystemTime>)>,
    pub storage_error: Option<String>,
    pub editor_error: Option<String>,
    inspect_id: Arc<AtomicU64>,
    inspecting: bool,
    loaded_document: Option<Arc<Document>>,
    pub scan_dirs: Vec<ScanDir>,
    pub midi_files: Vec<PathBuf>,
    pub search_query: String,
    pub selected_idx: Option<usize>,
    pub playing_idx: Option<usize>,
    playing_path: Option<PathBuf>,
    status: Arc<Mutex<PlaybackStatus>>,
    position_ms: Arc<AtomicU64>,
    total_ms: Arc<AtomicU64>,
    epoch: Arc<AtomicU64>,
    cmd_tx: mpsc::Sender<Cmd>,
    io_tx: mpsc::Sender<IoCmd>,
    notices: mpsc::Receiver<Notice>,
    scan_id: Arc<AtomicU64>,
    load_id: Arc<AtomicU64>,
    scanning: bool,
    loading: bool,
    workers: Vec<JoinHandle<()>>,
    last_error: Mutex<Option<String>>,
}

impl MidiPlayer {
    pub fn new(midi_tx: mpsc::SyncSender<MidiEvent>, epoch: Arc<AtomicU64>) -> Self {
        let root = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("miditoroblox");
        let store = Arc::new(Mutex::new(Store::new(root)));
        let worker_store = store.clone();
        let inspect_id = Arc::new(AtomicU64::new(0));
        let inspect_generation = inspect_id.clone();
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (io_tx, io_rx) = mpsc::channel();
        let (notice_tx, notices) = mpsc::channel();
        let status = Arc::new(Mutex::new(PlaybackStatus::Stopped));
        let position_ms = Arc::new(AtomicU64::new(0));
        let total_ms = Arc::new(AtomicU64::new(0));
        let scan_id = Arc::new(AtomicU64::new(0));
        let load_id = Arc::new(AtomicU64::new(0));
        let (s, p, e, n) = (
            status.clone(),
            position_ms.clone(),
            epoch.clone(),
            notice_tx.clone(),
        );
        let playback = thread::spawn(move || playback_thread(cmd_rx, midi_tx, s, p, e, n));
        let (scan_generation, load_generation) = (scan_id.clone(), load_id.clone());
        let io = thread::spawn(move || {
            while let Ok(cmd) = io_rx.recv() {
                match cmd {
                    IoCmd::Scan(id, dirs) => {
                        let (files, warnings) =
                            scan_files(&dirs, || scan_generation.load(Ordering::SeqCst) != id);
                        let _ = notice_tx.send(Notice::Scanned(id, files, warnings));
                    }
                    IoCmd::Load(id, path) => {
                        if load_generation.load(Ordering::SeqCst) == id {
                            let result = load_document(&path, &worker_store, &|| {
                                load_generation.load(Ordering::SeqCst) != id
                            });
                            let _ = notice_tx.send(Notice::Loaded(id, path, result));
                        }
                    }
                    IoCmd::Inspect(id, path) => {
                        if inspect_generation.load(Ordering::SeqCst) == id {
                            let result = load_document(&path, &worker_store, &|| {
                                inspect_generation.load(Ordering::SeqCst) != id
                            });
                            let _ = notice_tx.send(Notice::Inspected(id, path, result));
                        }
                    }
                    IoCmd::Shutdown => break,
                }
            }
        });
        Self {
            store,
            editor: None,
            editor_path: None,
            editor_stamp: None,
            storage_error: None,
            editor_error: None,
            inspect_id,
            inspecting: false,
            loaded_document: None,
            scan_dirs: Vec::new(),
            midi_files: Vec::new(),
            search_query: String::new(),
            selected_idx: None,
            playing_idx: None,
            playing_path: None,
            status,
            position_ms,
            total_ms,
            epoch,
            cmd_tx,
            io_tx,
            notices,
            scan_id,
            load_id,
            scanning: false,
            loading: false,
            workers: vec![playback, io],
            last_error: Mutex::new(None),
        }
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn busy(&self) -> bool {
        self.scanning || self.loading || self.inspecting
    }

    pub fn poll(&mut self) -> Option<String> {
        let mut message = self
            .last_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        while let Ok(notice) = self.notices.try_recv() {
            match notice {
                Notice::Scanned(id, files, warnings)
                    if id == self.scan_id.load(Ordering::SeqCst) =>
                {
                    let selected = self
                        .selected_idx
                        .and_then(|i| self.midi_files.get(i))
                        .cloned();
                    self.midi_files = files;
                    self.selected_idx =
                        selected.and_then(|p| self.midi_files.iter().position(|f| *f == p));
                    self.playing_idx = self
                        .playing_path
                        .as_ref()
                        .and_then(|p| self.midi_files.iter().position(|f| f == p));
                    self.scanning = false;
                    message = Some(if warnings.is_empty() {
                        format!("Found {} MIDI files", self.midi_files.len())
                    } else {
                        format!(
                            "Found {} MIDI files. {}",
                            self.midi_files.len(),
                            warnings.join("; ")
                        )
                    });
                }
                Notice::Loaded(id, path, result) if id == self.load_id.load(Ordering::SeqCst) => {
                    self.loading = false;
                    match result {
                        Ok((document, mut mutes, warning)) => {
                            // An edit made while the disk load was pending wins over its old profile.
                            if let Some(latest) = self
                                .store
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .latest(&Mutes::filename(&document.hash))
                            {
                                mutes = latest;
                            }
                            if let Some((_, current, edits)) = &self.editor
                                && current.hash == document.hash
                            {
                                mutes = edits.clone();
                            }
                            let timeline = document.timeline(&mutes);
                            self.loaded_document = Some(document.clone());
                            if self.editor_path.as_ref() == Some(&path) {
                                self.inspect_id.fetch_add(1, Ordering::SeqCst);
                                self.inspecting = false;
                                self.editor = Some((path.clone(), document, mutes));
                            }
                            self.total_ms
                                .store(timeline.total().as_millis() as u64, Ordering::Relaxed);
                            self.playing_idx = self.midi_files.iter().position(|p| *p == path);
                            self.playing_path = Some(path);
                            let epoch = self.invalidate();
                            self.command(Cmd::Load { timeline, epoch });
                            if let Some(error) = &warning {
                                self.storage_error = Some(error.clone());
                            }
                            message = Some(warning.unwrap_or_else(|| "Playing".into()));
                        }
                        Err(e) => message = Some(e),
                    }
                }
                Notice::Inspected(id, path, result)
                    if id == self.inspect_id.load(Ordering::SeqCst) =>
                {
                    self.inspecting = false;
                    match result {
                        Ok((document, mut mutes, warning)) => {
                            if let Some(latest) = self
                                .store
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .latest(&Mutes::filename(&document.hash))
                            {
                                mutes = latest;
                            }
                            self.editor = Some((path, document, mutes));
                            if let Some(error) = warning {
                                self.storage_error = Some(error.clone());
                                message = Some(error);
                            }
                        }
                        Err(e) => {
                            self.editor_error = Some(e.clone());
                            message = Some(e);
                        }
                    }
                }
                Notice::Error(e) => message = Some(e),
                _ => {}
            }
        }
        message
    }

    /// Selection inspection is independent of the loaded transport.
    pub fn inspect_selection(&mut self) {
        let path = self
            .selected_idx
            .and_then(|i| self.midi_files.get(i))
            .cloned();
        let stamp = path
            .as_ref()
            .and_then(|path| std::fs::metadata(path).ok())
            .map(|m| (m.len(), m.modified().ok()));
        if path == self.editor_path && stamp == self.editor_stamp {
            return;
        }
        self.editor_stamp = stamp;
        self.editor_error = None;
        self.editor_path = path.clone();
        self.editor = None;
        let id = self.inspect_id.fetch_add(1, Ordering::SeqCst) + 1;
        self.inspecting = path.is_some();
        if let Some(path) = path
            && self.io_tx.send(IoCmd::Inspect(id, path)).is_err()
        {
            self.inspecting = false;
            *self.last_error.lock().unwrap_or_else(|e| e.into_inner()) =
                Some("MIDI inspection worker is unavailable".into());
        }
    }
    pub fn apply_mutes(&mut self, mutes: Mutes) {
        let Some((_, document, current)) = &mut self.editor else {
            return;
        };
        *current = mutes.clone();
        let document = document.clone();
        self.store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .queue(&Mutes::filename(&document.hash), &mutes);
        if self
            .loaded_document
            .as_ref()
            .is_some_and(|loaded| loaded.hash == document.hash)
        {
            self.command(Cmd::Replace {
                timeline: document.timeline(&mutes),
                position: Duration::from_secs_f64(self.position()),
                epoch: self.invalidate(),
            });
        }
    }

    pub fn scan(&mut self) {
        let id = self.scan_id.fetch_add(1, Ordering::SeqCst) + 1;
        self.scanning = true;
        if self
            .io_tx
            .send(IoCmd::Scan(id, self.scan_dirs.clone()))
            .is_err()
        {
            self.scanning = false;
            *self.last_error.lock().unwrap_or_else(|e| e.into_inner()) =
                Some("File scanning worker is unavailable".into());
        }
    }

    pub fn play(&mut self, idx: usize) {
        let Some(path) = self.midi_files.get(idx).cloned() else {
            return;
        };
        self.stop();
        self.selected_idx = Some(idx);
        let id = self.load_id.fetch_add(1, Ordering::SeqCst) + 1;
        self.loading = true;
        if self.io_tx.send(IoCmd::Load(id, path)).is_err() {
            self.loading = false;
            *self.last_error.lock().unwrap_or_else(|e| e.into_inner()) =
                Some("MIDI loading worker is unavailable".into());
        }
    }

    fn invalidate(&self) -> u64 {
        self.epoch.fetch_add(1, Ordering::SeqCst) + 1
    }
    fn command(&self, cmd: Cmd) {
        if self.cmd_tx.send(cmd).is_err() {
            *self.last_error.lock().unwrap_or_else(|e| e.into_inner()) =
                Some("Playback worker is unavailable".into());
            *self.status.lock().unwrap_or_else(|e| e.into_inner()) = PlaybackStatus::Stopped;
        }
    }
    pub fn pause(&self) {
        self.command(Cmd::Pause(self.invalidate()));
    }
    pub fn resume(&self) {
        self.command(Cmd::Resume(self.invalidate()));
    }
    pub fn seek(&self, seconds: f64) {
        if seconds.is_finite() && seconds >= 0.0 {
            self.command(Cmd::Seek(
                Duration::from_secs_f64(seconds.min(self.total())),
                self.invalidate(),
            ));
        }
    }
    pub fn stop(&mut self) {
        self.load_id.fetch_add(1, Ordering::SeqCst);
        self.loading = false;
        self.playing_idx = None;
        self.playing_path = None;
        self.loaded_document = None;
        self.command(Cmd::Stop(self.invalidate()));
    }
    pub fn status(&self) -> PlaybackStatus {
        *self.status.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn position(&self) -> f64 {
        self.position_ms.load(Ordering::Relaxed) as f64 / 1000.0
    }
    pub fn total(&self) -> f64 {
        self.total_ms.load(Ordering::Relaxed) as f64 / 1000.0
    }
    pub fn filtered_files(&self) -> Vec<(usize, PathBuf)> {
        let q = self.search_query.to_lowercase();
        self.midi_files
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                q.is_empty()
                    || p.file_name()
                        .is_some_and(|name| name.to_string_lossy().to_lowercase().contains(&q))
            })
            .map(|(i, p)| (i, p.clone()))
            .collect()
    }
    pub fn shutdown(&mut self) {
        if self.workers.is_empty() {
            return;
        }
        self.scan_id.fetch_add(1, Ordering::SeqCst);
        self.inspect_id.fetch_add(1, Ordering::SeqCst);
        self.load_id.fetch_add(1, Ordering::SeqCst);
        self.invalidate();
        let _ = self.cmd_tx.send(Cmd::Shutdown);
        let _ = self.io_tx.send(IoCmd::Shutdown);
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}
impl Drop for MidiPlayer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn scan_files(dirs: &[ScanDir], cancelled: impl Fn() -> bool) -> (Vec<PathBuf>, Vec<String>) {
    let mut files = HashSet::new();
    let mut visited = HashSet::new();
    let mut warnings = Vec::new();
    for dir in dirs {
        let mut pending = vec![dir.path.clone()];
        // Each root gets its own visit set so a nonrecursive root does not suppress a recursive one.
        visited.clear();
        while let Some(path) = pending.pop() {
            if cancelled() {
                return (Vec::new(), Vec::new());
            }
            let canonical = match path.canonicalize() {
                Ok(p) => p,
                Err(e) => {
                    warnings.push(format!("{}: {e}", path.display()));
                    continue;
                }
            };
            if !visited.insert(canonical.clone()) {
                continue;
            }
            let entries = match std::fs::read_dir(&canonical) {
                Ok(entries) => entries,
                Err(e) => {
                    warnings.push(format!("{}: {e}", path.display()));
                    continue;
                }
            };
            for entry in entries {
                if cancelled() {
                    return (Vec::new(), Vec::new());
                }
                let entry = match entry {
                    Ok(e) => e,
                    Err(e) => {
                        warnings.push(e.to_string());
                        continue;
                    }
                };
                let p = entry.path();
                if p.is_dir() && dir.recursive {
                    pending.push(p);
                } else if p.is_file()
                    && p.extension().is_some_and(|e| {
                        e.eq_ignore_ascii_case("mid") || e.eq_ignore_ascii_case("midi")
                    })
                {
                    match p.canonicalize() {
                        Ok(p) => {
                            files.insert(p);
                        }
                        Err(e) => warnings.push(e.to_string()),
                    }
                }
            }
        }
    }
    let mut files: Vec<_> = files.into_iter().collect();
    files.sort_by(|a, b| a.file_name().cmp(&b.file_name()).then_with(|| a.cmp(b)));
    (files, warnings)
}

#[cfg(test)]
fn parse_midi_bytes(data: &[u8]) -> Result<(Vec<Event>, u32), String> {
    let doc = Document::parse(data, &|| false)?;
    Ok((doc.events, doc.tpb))
}
impl Document {
    fn parse(data: &[u8], cancelled: &impl Fn() -> bool) -> Result<Self, String> {
        let smf = midly::Smf::parse(data).map_err(|e| e.to_string())?;
        if smf.header.format == midly::Format::Sequential {
            return Err("Sequential MIDI tracks are not supported".into());
        }
        let tpb = match smf.header.timing {
            midly::Timing::Metrical(t) if t.as_int() > 0 => t.as_int() as u32,
            _ => return Err("Unsupported MIDI time division".into()),
        };
        let mut events = Vec::new();
        let mut end = 0;
        let mut tracks = Vec::new();
        let mut names = Vec::new();
        for (track_index, track) in smf.tracks.iter().enumerate() {
            let mut name = String::new();
            let mut abs = 0;
            for ev in track {
                if cancelled() {
                    return Err("Detection cancelled".into());
                }
                abs += ev.delta.as_int() as u64;
                if let midly::TrackEventKind::Meta(midly::MetaMessage::TrackName(bytes)) = ev.kind {
                    name = String::from_utf8_lossy(bytes).into_owned();
                }
                let kind = match ev.kind {
                    midly::TrackEventKind::Midi { channel, message } => {
                        let ch = channel.as_int();
                        let bytes = match message {
                            midly::MidiMessage::NoteOn { key, vel } => {
                                Some(vec![0x90 | ch, key.as_int(), vel.as_int()])
                            }
                            midly::MidiMessage::NoteOff { key, vel } => {
                                Some(vec![0x80 | ch, key.as_int(), vel.as_int()])
                            }
                            midly::MidiMessage::Controller { controller, value } => {
                                Some(vec![0xB0 | ch, controller.as_int(), value.as_int()])
                            }
                            midly::MidiMessage::ProgramChange { program } => {
                                Some(vec![0xC0 | ch, program.as_int()])
                            }
                            _ => None,
                        };
                        bytes.map(EventKind::Midi)
                    }
                    midly::TrackEventKind::Meta(midly::MetaMessage::Tempo(t)) => {
                        Some(EventKind::Tempo(t.as_int()))
                    }
                    _ => None,
                };
                if let Some(kind) = kind {
                    events.push(Event {
                        abs_tick: abs,
                        kind,
                    });
                    tracks.push(track_index);
                }
            }
            end = end.max(abs);
            names.push(name);
        }
        events.push(Event {
            abs_tick: end,
            kind: EventKind::End,
        });
        tracks.push(0);
        let mut merged: Vec<_> = events.into_iter().zip(tracks).collect();
        merged.sort_by_key(|(e, _)| e.abs_tick);
        let mut programs = [0u8; 16];
        let mut rows: BTreeMap<InstrumentId, InstrumentRow> = BTreeMap::new();
        // FIFO preserves ownership even when a release follows a program change.
        let mut held: BTreeMap<(u8, u8), Vec<(usize, InstrumentId)>> = BTreeMap::new();
        let mut owners = Vec::with_capacity(merged.len());
        for (event, track) in &merged {
            if cancelled() {
                return Err("Detection cancelled".into());
            }
            let mut owner = None;
            if let EventKind::Midi(bytes) = &event.kind {
                let channel = bytes[0] & 15;
                match bytes[0] & 0xf0 {
                    0xc0 => programs[channel as usize] = bytes[1],
                    0x90 if bytes[2] > 0 => {
                        let id = InstrumentId {
                            track: *track,
                            program: if channel == 9 {
                                0
                            } else {
                                programs[channel as usize]
                            },
                            percussion: channel == 9,
                        };
                        let row = rows.entry(id).or_insert_with(|| InstrumentRow {
                            id,
                            track_name: names[*track].clone(),
                            channels: BTreeSet::new(),
                            notes: 0,
                        });
                        row.channels.insert(channel + 1);
                        row.notes += 1;
                        held.entry((channel, bytes[1]))
                            .or_default()
                            .push((*track, id));
                        owner = Some(id);
                    }
                    0x80 | 0x90 => {
                        if let Some(queue) = held.get_mut(&(channel, bytes[1]))
                            && !queue.is_empty()
                        {
                            let index = queue.iter().position(|(t, _)| t == track).unwrap_or(0);
                            owner = Some(queue.remove(index).1);
                        }
                    }
                    0xb0 if bytes[1] == 120 || bytes[1] == 123 => {
                        held.retain(|(ch, _), _| *ch != channel);
                    }
                    _ => {}
                }
            }
            owners.push(owner);
        }
        Ok(Document {
            hash: format!("{:x}", Sha256::digest(data)),
            instruments: rows.into_values().collect(),
            events: merged.into_iter().map(|(e, _)| e).collect(),
            owners,
            tpb,
        })
    }
}

/// Analysis uses exactly the same parsing, ordering, and tempo conversion as playback.
pub(crate) struct AnalysisMidi {
    pub events: Vec<(Duration, Vec<u8>)>,
    pub total: Duration,
}

#[cfg(test)]
pub(crate) fn analysis_events(
    path: &Path,
    cancelled: &impl Fn() -> bool,
) -> Result<AnalysisMidi, String> {
    if cancelled() {
        return Err("Detection cancelled".into());
    }
    analysis_events_edited(path, &Mutes::default(), None, cancelled)
}

pub(crate) fn analysis_events_edited(
    path: &Path,
    mutes: &Mutes,
    hash: Option<&str>,
    cancelled: &impl Fn() -> bool,
) -> Result<AnalysisMidi, String> {
    let document = Document::load(path, cancelled)?;
    if hash.is_some_and(|hash| hash != document.hash) {
        return Err("MIDI contents changed; select the file again".into());
    }
    let timeline = document.timeline(mutes);
    let total = timeline.total();
    let events = timeline
        .events
        .into_iter()
        .zip(timeline.deadlines)
        .filter_map(|(event, due)| {
            if let EventKind::Midi(bytes) = event.kind {
                Some((due, bytes))
            } else {
                None
            }
        })
        .collect();
    Ok(AnalysisMidi { events, total })
}

/// Monotonic, tempo-aware song clock. Duration is independent of event density.
struct Timeline {
    events: Vec<Event>,
    deadlines: Vec<Duration>,
    cursor: usize,
    position: Duration,
    anchor: Option<Duration>,
}
impl Timeline {
    fn new(events: Vec<Event>, tpb: u32, now: Duration) -> Self {
        let mut deadlines = Vec::with_capacity(events.len());
        let (mut tick, mut tempo, mut numerator) = (0, 500_000u128, 0u128);
        for event in &events {
            numerator += event.abs_tick.saturating_sub(tick) as u128 * tempo * 1000;
            deadlines.push(Duration::from_nanos(
                (numerator / tpb.max(1) as u128).min(u64::MAX as u128) as u64,
            ));
            tick = event.abs_tick;
            if let EventKind::Tempo(value) = event.kind {
                tempo = value.max(1) as u128;
            }
        }
        let anchor = (!events.is_empty()).then_some(now);
        Self {
            events,
            deadlines,
            cursor: 0,
            position: Duration::ZERO,
            anchor,
        }
    }
    fn total(&self) -> Duration {
        self.deadlines.last().copied().unwrap_or_default()
    }
    fn elapsed(&self, now: Duration) -> Duration {
        (self.position
            + self
                .anchor
                .map(|a| now.saturating_sub(a))
                .unwrap_or_default())
        .min(self.total())
    }
    fn pause(&mut self, now: Duration) {
        self.position = self.elapsed(now);
        self.anchor = None;
    }
    fn resume(&mut self, now: Duration) {
        if self.anchor.is_none() && self.cursor < self.events.len() {
            self.anchor = Some(now);
        }
    }
    fn seek(&mut self, time: Duration, now: Duration) {
        self.position = time.min(self.total());
        self.cursor = self.deadlines.partition_point(|d| *d < self.position);
        if self.anchor.is_some() {
            self.anchor = Some(now);
        }
    }
    fn wait(&self, now: Duration) -> Option<Duration> {
        self.deadlines.get(self.cursor).map(|due| {
            due.saturating_sub(self.elapsed(now))
                + self
                    .anchor
                    .map(|a| a.saturating_sub(now))
                    .unwrap_or_default()
        })
    }
    fn pop_due(&mut self, now: Duration) -> Option<(Event, Duration)> {
        let anchor = self.anchor?;
        if now < anchor || !self.wait(now)?.is_zero() {
            return None;
        }
        let due = anchor + self.deadlines[self.cursor].saturating_sub(self.position);
        let event = self.events[self.cursor].clone();
        self.cursor += 1;
        Some((event, due))
    }
    fn restore(&self) -> Vec<Vec<u8>> {
        let mut programs = [0; 16];
        let mut sustain = [false; 16];
        let mut notes = BTreeMap::new();
        for event in &self.events[..self.cursor] {
            let EventKind::Midi(b) = &event.kind else {
                continue;
            };
            let ch = (b[0] & 15) as usize;
            match b[0] & 0xF0 {
                0xC0 => programs[ch] = b[1],
                0xB0 if b[1] == 64 => {
                    sustain[ch] = b[2] >= 64;
                    if !sustain[ch] {
                        notes.retain(|&(c, _), &mut (_, held)| c != ch || held);
                    }
                }
                0xB0 if b[1] == 120 => {
                    notes.retain(|&(c, _), _| c != ch);
                }
                0xB0 if b[1] == 123 => {
                    if sustain[ch] {
                        for ((c, _), (_, held)) in &mut notes {
                            if *c == ch {
                                *held = false;
                            }
                        }
                    } else {
                        notes.retain(|&(c, _), _| c != ch);
                    }
                }
                0x90 if b[2] > 0 => {
                    notes.insert((ch, b[1]), (b[2], true));
                }
                0x80 | 0x90 => {
                    if sustain[ch] {
                        if let Some((_, held)) = notes.get_mut(&(ch, b[1])) {
                            *held = false;
                        }
                    } else {
                        notes.remove(&(ch, b[1]));
                    }
                }
                _ => {}
            }
        }
        let mut messages = Vec::new();
        for (ch, program) in programs.into_iter().enumerate() {
            messages.push(vec![0xC0 | ch as u8, program]);
        }
        for (ch, down) in sustain.into_iter().enumerate() {
            messages.push(vec![0xB0 | ch as u8, 64, if down { 127 } else { 0 }]);
        }
        for ((ch, key), (vel, held)) in notes {
            messages.push(vec![0x90 | ch as u8, key, vel]);
            if !held {
                messages.push(vec![0x80 | ch as u8, key, 0]);
            }
        }
        messages
    }
}

fn playback_thread(
    cmd_rx: mpsc::Receiver<Cmd>,
    midi_tx: mpsc::SyncSender<MidiEvent>,
    status: Arc<Mutex<PlaybackStatus>>,
    position_ms: Arc<AtomicU64>,
    epoch: Arc<AtomicU64>,
    notices: mpsc::Sender<Notice>,
) {
    let clock = Instant::now();
    let mut timeline = Timeline::new(Vec::new(), 480, Duration::ZERO);
    let mut generation = epoch.load(Ordering::SeqCst);
    let mut restore_pending = false;
    // Preserve command intent while epoch invalidation temporarily pauses stale work.
    let mut transport_status = PlaybackStatus::Stopped;
    let set_status = |s| *status.lock().unwrap_or_else(|e| e.into_inner()) = s;
    loop {
        let timeout = if timeline.anchor.is_some() {
            timeline
                .wait(clock.elapsed())
                .unwrap_or_default()
                .min(Duration::from_millis(10))
        } else {
            Duration::from_millis(10)
        };
        match cmd_rx.recv_timeout(timeout) {
            Ok(Cmd::Load {
                timeline: song,
                epoch: next,
            }) => {
                timeline = song;
                timeline.anchor = Some(clock.elapsed() + FOCUS_DELAY);
                generation = next;
                restore_pending = true;
                transport_status = PlaybackStatus::Playing;
                set_status(transport_status);
            }
            Ok(Cmd::Pause(next)) => {
                timeline.pause(clock.elapsed());
                generation = next;
                if timeline.cursor < timeline.events.len() {
                    transport_status = PlaybackStatus::Paused;
                    set_status(transport_status);
                }
            }
            Ok(Cmd::Resume(next)) => {
                timeline.resume(clock.elapsed() + FOCUS_DELAY);
                generation = next;
                restore_pending = true;
                if timeline.anchor.is_some() {
                    transport_status = PlaybackStatus::Playing;
                    set_status(transport_status);
                }
            }
            Ok(Cmd::Replace {
                timeline: mut song,
                epoch: next,
                position,
            }) => {
                let playing = transport_status == PlaybackStatus::Playing;
                song.seek(position, clock.elapsed() + FOCUS_DELAY);
                if !playing {
                    song.anchor = None;
                }
                timeline = song;
                generation = next;
                restore_pending = true;
                set_status(transport_status);
            }
            Ok(Cmd::Stop(next)) => {
                timeline = Timeline::new(Vec::new(), 480, Duration::ZERO);
                generation = next;
                restore_pending = false;
                transport_status = PlaybackStatus::Stopped;
                set_status(transport_status);
            }
            Ok(Cmd::Seek(time, next)) => {
                timeline.seek(time, clock.elapsed() + FOCUS_DELAY);
                generation = next;
                restore_pending = true;
            }
            #[cfg(test)]
            Ok(Cmd::Barrier(reply)) => {
                let _ = reply.send(());
            }
            Ok(Cmd::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        // Never stamp work from an old transport state with the new epoch.
        if generation != epoch.load(Ordering::SeqCst) {
            timeline.pause(clock.elapsed());
            if timeline.cursor < timeline.events.len() {
                set_status(PlaybackStatus::Paused);
            }
        }
        position_ms.store(
            timeline.elapsed(clock.elapsed()).as_millis() as u64,
            Ordering::Relaxed,
        );
        let mut output = Vec::new();
        if let Some(anchor) = timeline.anchor.filter(|a| clock.elapsed() >= *a) {
            if restore_pending {
                let at = clock + anchor;
                output.push(MidiEvent::reset(MidiSource::Player, generation, at));
                output.extend(
                    timeline
                        .restore()
                        .into_iter()
                        .map(|b| MidiEvent::at(b, generation, at)),
                );
                restore_pending = false;
            }
            while let Some((event, due)) = timeline.pop_due(clock.elapsed()) {
                if let EventKind::Midi(bytes) = event.kind {
                    output.push(MidiEvent::at(bytes, generation, clock + due));
                }
            }
            if timeline.cursor >= timeline.events.len() {
                // Cleanup follows the final events at the same deadline; do not invalidate them.
                let at = clock + anchor + timeline.total().saturating_sub(timeline.position);
                for ch in 0..16 {
                    output.push(MidiEvent::at(vec![0xB0 | ch, 64, 0], generation, at));
                    output.push(MidiEvent::at(vec![0xB0 | ch, 123, 0], generation, at));
                }
                timeline.pause(clock.elapsed());
                transport_status = PlaybackStatus::Stopped;
                set_status(transport_status);
            }
        }
        for event in output {
            if midi_tx.try_send(event).is_err() {
                epoch.fetch_add(1, Ordering::SeqCst);
                timeline.pause(clock.elapsed());
                transport_status = PlaybackStatus::Paused;
                set_status(transport_status);
                let _ = notices.send(Notice::Error(
                    "MIDI output queue is full or unavailable; playback paused".into(),
                ));
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }
    fn note(tick: u64) -> Event {
        Event {
            abs_tick: tick,
            kind: EventKind::Midi(vec![0x90, 60, 100]),
        }
    }

    fn fixture(tracks: &[Vec<(u32, &[u8])>]) -> Vec<u8> {
        use midly::{
            Format, Header, MetaMessage, Smf, Timing, TrackEvent, TrackEventKind, live::LiveEvent,
        };
        let tracks = tracks
            .iter()
            .map(|events| {
                let mut track = vec![TrackEvent {
                    delta: 0.into(),
                    kind: TrackEventKind::Meta(MetaMessage::TrackName(b"Part")),
                }];
                let mut last = 0;
                for (tick, bytes) in events {
                    let kind = if bytes[0] == 0xff {
                        TrackEventKind::Meta(MetaMessage::Tempo(1_000_000.into()))
                    } else {
                        match LiveEvent::parse(bytes).unwrap() {
                            LiveEvent::Midi { channel, message } => {
                                TrackEventKind::Midi { channel, message }
                            }
                            _ => unreachable!(),
                        }
                    };
                    track.push(TrackEvent {
                        delta: (tick - last).into(),
                        kind,
                    });
                    last = *tick;
                }
                track.push(TrackEvent {
                    delta: (1920 - last).into(),
                    kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
                });
                track
            })
            .collect();
        let smf = Smf {
            header: Header::new(Format::Parallel, Timing::Metrical(480.into())),
            tracks,
        };
        let mut bytes = Vec::new();
        smf.write_std(&mut bytes).unwrap();
        bytes
    }
    fn id(track: usize, program: u8) -> InstrumentId {
        InstrumentId {
            track,
            program,
            percussion: false,
        }
    }
    fn messages(timeline: &Timeline) -> Vec<Vec<u8>> {
        timeline
            .events
            .iter()
            .filter_map(|e| {
                if let EventKind::Midi(b) = &e.kind {
                    Some(b.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    #[test]
    fn editor_groups_by_track_program_and_percussion_using_merged_channel_state() {
        let bytes = fixture(&[
            vec![
                (0, &[0xc0, 40]),
                (0, &[0x90, 60, 100]),
                (0, &[0x99, 36, 100]),
                (120, &[0xc0, 0]),
                (120, &[0x90, 64, 100]),
                (240, &[0x91, 65, 100]),
                (360, &[0xc9, 10]),
                (360, &[0x99, 38, 100]),
            ],
            vec![(0, &[0x90, 67, 100]), (240, &[0x80, 67, 0])],
        ]);
        let document = Document::parse(&bytes, &|| false).unwrap();
        assert_eq!(document.instruments.len(), 4);
        let piano = document
            .instruments
            .iter()
            .find(|r| r.id == id(0, 0))
            .unwrap();
        assert_eq!(piano.channels, BTreeSet::from([1, 2]));
        assert_eq!(piano.track_name, "Part");
        assert_eq!(piano.notes, 2);
        assert!(document.instruments.iter().any(|r| r.id == id(1, 40)));
        assert_eq!(
            document
                .instruments
                .iter()
                .find(|r| r.id.percussion)
                .unwrap()
                .notes,
            2
        );
        let muted = Mutes {
            muted: BTreeSet::from([id(0, 40)]),
        };
        let filtered = messages(&document.timeline(&muted));
        assert!(!filtered.contains(&vec![0x90, 60, 100]));
        assert!(filtered.contains(&vec![0x90, 67, 100]));
    }

    #[test]
    fn release_ownership_survives_program_changes_repeats_and_cross_track_releases() {
        let bytes = fixture(&[
            vec![
                (0, &[0x90, 60, 100]),
                (0, &[0x90, 60, 90]),
                (120, &[0xc0, 40]),
                (120, &[0x90, 60, 80]),
                (240, &[0x90, 60, 0]),
                (360, &[0x80, 60, 0]),
            ],
            vec![(480, &[0x80, 60, 1])],
        ]);
        let document = Document::parse(&bytes, &|| false).unwrap();
        let muted = Mutes {
            muted: BTreeSet::from([id(0, 0)]),
        };
        assert_eq!(
            messages(&document.timeline(&muted)),
            vec![vec![0xc0, 40], vec![0x90, 60, 80], vec![0x80, 60, 1]]
        );
    }

    #[test]
    fn local_release_matching_wins_over_older_note_in_another_track() {
        let bytes = fixture(&[
            vec![(0, &[0x90, 60, 100])],
            vec![(120, &[0x90, 60, 90]), (240, &[0x80, 60, 1])],
            vec![(360, &[0x80, 60, 2])],
        ]);
        let document = Document::parse(&bytes, &|| false).unwrap();
        let muted = Mutes {
            muted: BTreeSet::from([id(1, 0)]),
        };
        assert_eq!(
            messages(&document.timeline(&muted)),
            vec![vec![0x90, 60, 100], vec![0x80, 60, 2]]
        );
    }

    #[test]
    fn playback_analysis_seek_and_silence_use_the_same_filtered_timeline() {
        let dir = TestDir::new();
        let path = dir.0.join("song.mid");
        let bytes = fixture(&[vec![
            (0, &[0xb0, 64, 127]),
            (0, &[0x90, 60, 100]),
            (240, &[0x80, 60, 0]),
            (480, &[0xff]),
            (480, &[0xc1, 40]),
            (480, &[0x91, 70, 100]),
            (960, &[0x81, 70, 0]),
            (960, &[0xb0, 64, 0]),
        ]]);
        std::fs::write(&path, &bytes).unwrap();
        let document = Document::parse(&bytes, &|| false).unwrap();
        let muted = Mutes {
            muted: BTreeSet::from([id(0, 0)]),
        };
        let mut playback = document.timeline(&muted);
        let analysis =
            analysis_events_edited(&path, &muted, Some(&document.hash), &|| false).unwrap();
        let expected: Vec<_> = playback
            .events
            .iter()
            .zip(&playback.deadlines)
            .filter_map(|(e, d)| {
                if let EventKind::Midi(b) = &e.kind {
                    Some((*d, b.clone()))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(analysis.events, expected);
        assert_eq!(analysis.total, ms(3500));
        playback.seek(ms(750), ms(0));
        let restored = playback.restore();
        assert!(!restored.iter().any(|b| b[0] == 0x90));
        assert!(restored.contains(&vec![0x91, 70, 100]));
        assert!(restored.contains(&vec![0xb0, 64, 127]));
        let all_muted = Mutes {
            muted: document.instruments.iter().map(|r| r.id).collect(),
        };
        let silent = document.timeline(&all_muted);
        assert_eq!(silent.total(), analysis.total);
        assert!(
            !messages(&silent)
                .iter()
                .any(|b| b[0] & 0xf0 == 0x90 && b[2] > 0)
        );
        assert!(analysis_events_edited(&path, &muted, Some("wrong hash"), &|| false).is_err());
    }

    #[test]
    fn content_identity_shares_profiles_across_paths_but_separates_changed_files() {
        let dir = TestDir::new();
        let first = dir.0.join("first.mid");
        let moved = dir.0.join("moved.mid");
        let bytes = fixture(&[vec![(0, &[0x90, 60, 100])]]);
        std::fs::write(&first, &bytes).unwrap();
        std::fs::write(&moved, &bytes).unwrap();
        let store = Mutex::new(Store::new(dir.0.join("working-directory/miditoroblox")));
        let (one, _, _) = load_document(&first, &store, &|| false).unwrap();
        let mutes = Mutes {
            muted: BTreeSet::from([id(0, 0)]),
        };
        store
            .lock()
            .unwrap()
            .queue(&Mutes::filename(&one.hash), &mutes);
        let (two, restored, _) = load_document(&moved, &store, &|| false).unwrap();
        assert_eq!(one.hash, two.hash);
        assert_eq!(restored, mutes); // Pending edits also follow copies before disk flush.
        assert!(store.lock().unwrap().flush(true).is_empty());
        std::fs::write(&first, fixture(&[vec![(0, &[0x90, 61, 100])]])).unwrap();
        let (changed, restored, _) = load_document(&first, &store, &|| false).unwrap();
        assert_ne!(changed.hash, one.hash);
        assert_eq!(restored, Mutes::default());
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 3); // No MIDI sidecars.
    }

    #[test]
    fn stale_inspection_and_load_results_cannot_replace_the_current_editor() {
        let (tx, _rx) = mpsc::sync_channel(4096);
        let mut player = MidiPlayer::new(tx, Arc::new(AtomicU64::new(0)));
        let document =
            Arc::new(Document::parse(&fixture(&[vec![(0, &[0x90, 60, 100])]]), &|| false).unwrap());
        let current = PathBuf::from("current.mid");
        player.editor = Some((current.clone(), document.clone(), Mutes::default()));
        player.inspect_id.store(2, Ordering::SeqCst);
        player.load_id.store(2, Ordering::SeqCst);
        let (notice_tx, notice_rx) = mpsc::channel();
        player.notices = notice_rx;
        notice_tx
            .send(Notice::Inspected(
                1,
                PathBuf::from("old.mid"),
                Ok((document.clone(), Mutes::default(), None)),
            ))
            .unwrap();
        notice_tx
            .send(Notice::Loaded(
                1,
                PathBuf::from("old.mid"),
                Ok((document, Mutes::default(), None)),
            ))
            .unwrap();
        player.poll();
        assert_eq!(player.editor.as_ref().unwrap().0, current);
        assert!(player.loaded_document.is_none());
    }

    #[test]
    fn mute_edits_preserve_transport_and_position_and_leave_other_loaded_content_alone() {
        let dir = TestDir::new();
        let (tx, rx) = mpsc::sync_channel(4096);
        let epoch = Arc::new(AtomicU64::new(0));
        let mut player = MidiPlayer::new(tx, epoch.clone());
        player.store = Arc::new(Mutex::new(Store::new(dir.0.join("saved"))));
        let document =
            Arc::new(Document::parse(&fixture(&[vec![(0, &[0x90, 60, 100])]]), &|| false).unwrap());
        let other =
            Arc::new(Document::parse(&fixture(&[vec![(0, &[0x90, 61, 100])]]), &|| false).unwrap());
        player.loaded_document = Some(document.clone());
        player.editor = Some((PathBuf::from("other.mid"), other, Mutes::default()));
        let mutes = Mutes {
            muted: BTreeSet::from([id(0, 0)]),
        };
        player.apply_mutes(mutes.clone());
        assert_eq!(epoch.load(Ordering::SeqCst), 0);
        player.editor = Some((
            PathBuf::from("song.mid"),
            document.clone(),
            Mutes::default(),
        ));
        for status in [
            PlaybackStatus::Paused,
            PlaybackStatus::Playing,
            PlaybackStatus::Stopped,
        ] {
            player.command(Cmd::Load {
                timeline: document.timeline(&Mutes::default()),
                epoch: epoch.load(Ordering::SeqCst),
            });
            match status {
                PlaybackStatus::Paused => player.pause(),
                PlaybackStatus::Stopped => player.command(Cmd::Stop(epoch.load(Ordering::SeqCst))),
                PlaybackStatus::Playing => {}
            }
            let sync = || {
                let (tx, rx) = mpsc::channel();
                player.command(Cmd::Barrier(tx));
                rx.recv_timeout(Duration::from_secs(2)).unwrap();
            };
            sync();
            player.total_ms.store(2000, Ordering::Relaxed);
            let position = if status == PlaybackStatus::Stopped {
                0.0
            } else {
                0.5
            };
            player.seek(position);
            sync();
            player.apply_mutes(mutes.clone());
            let generation = epoch.load(Ordering::SeqCst);
            let (ack, ack_rx) = mpsc::channel();
            player.command(Cmd::Barrier(ack));
            ack_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(player.status(), status);
            if status == PlaybackStatus::Playing {
                let reset = rx.recv_timeout(Duration::from_secs(2)).unwrap();
                assert_eq!(reset.epoch, generation);
                assert!(reset.reset);
            }
            assert!((position..position + 0.15).contains(&player.position()));
            player.command(Cmd::Pause(generation));
            let (ack, ack_rx) = mpsc::channel();
            player.command(Cmd::Barrier(ack));
            ack_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            while rx.try_recv().is_ok() {}
        }
        let stale = MidiEvent::at(vec![0x90, 60, 100], 0, Instant::now());
        assert!(!stale.allowed(epoch.load(Ordering::SeqCst), false, false, false));
    }

    #[test]
    fn initial_rest_is_preserved() {
        let mut t = Timeline::new(vec![note(480), note(960)], 480, ms(100));
        assert_eq!(t.wait(ms(100)), Some(ms(500)));
        assert!(t.pop_due(ms(599)).is_none());
        assert_eq!(t.pop_due(ms(600)).unwrap().0.abs_tick, 480);
    }

    #[test]
    fn repeated_pause_resume_preserves_the_remaining_wait() {
        let mut t = Timeline::new(vec![note(480)], 480, ms(0));
        t.pause(ms(100));
        t.pause(ms(900));
        t.resume(ms(1000));
        t.resume(ms(1100));
        assert_eq!(t.wait(ms(1100)), Some(ms(300)));
        t.pause(ms(1200));
        t.resume(ms(2000));
        assert_eq!(t.wait(ms(2000)), Some(ms(200)));
        assert!(t.pop_due(ms(2199)).is_none());
        assert!(t.pop_due(ms(2200)).is_some());
    }

    #[test]
    fn seek_preserves_the_gap_and_paused_state() {
        let mut t = Timeline::new(vec![note(0), note(480), note(960)], 480, ms(0));
        t.seek(ms(250), ms(1000));
        assert_eq!(t.wait(ms(1000)), Some(ms(250)));
        assert!(t.pop_due(ms(1249)).is_none());
        assert_eq!(t.pop_due(ms(1250)).unwrap().0.abs_tick, 480);
        t.pause(ms(1250));
        t.seek(ms(750), ms(2000));
        assert!(t.pop_due(ms(9000)).is_none());
        t.resume(ms(10000));
        assert_eq!(t.wait(ms(10000)), Some(ms(250)));
    }

    #[test]
    fn tempo_changes_are_integrated_for_playback_and_seek() {
        let mut t = Timeline::new(
            vec![
                note(0),
                Event {
                    abs_tick: 480,
                    kind: EventKind::Tempo(1_000_000),
                },
                note(960),
            ],
            480,
            ms(0),
        );
        assert_eq!(t.deadlines, vec![ms(0), ms(500), ms(1500)]);
        t.seek(ms(1000), ms(2000));
        assert_eq!(t.wait(ms(2000)), Some(ms(500)));
        t.seek(ms(500), ms(3000));
        assert_eq!(t.wait(ms(3000)), Some(ms(0)));
        t.pop_due(ms(3000));
        assert_eq!(t.wait(ms(3000)), Some(ms(1000)));
    }

    #[test]
    fn processing_delay_does_not_shift_deadlines() {
        let mut t = Timeline::new(vec![note(0), note(480), note(960)], 480, ms(0));
        assert!(t.pop_due(ms(30)).is_some());
        assert_eq!(t.wait(ms(30)), Some(ms(470)));
        assert!(t.pop_due(ms(570)).is_some());
        assert_eq!(t.wait(ms(570)), Some(ms(430)));
        assert!(t.pop_due(ms(1000)).is_some());
    }

    #[test]
    fn fractional_tick_durations_do_not_accumulate_rounding_error() {
        let events = (0..=960).map(note).collect();
        let t = Timeline::new(events, 960, ms(0));
        assert_eq!(t.deadlines.last(), Some(&ms(500)));
    }

    #[test]
    fn empty_timeline_and_resume_stay_stopped() {
        let mut t = Timeline::new(vec![], 480, ms(0));
        t.resume(ms(100));
        assert!(t.anchor.is_none());
        assert!(t.pop_due(ms(100)).is_none());
        assert!(t.wait(ms(100)).is_none());
    }

    fn midi(tick: u64, bytes: &[u8]) -> Event {
        Event {
            abs_tick: tick,
            kind: EventKind::Midi(bytes.to_vec()),
        }
    }

    #[test]
    fn progress_advances_through_rests_and_honors_tempo_changes() {
        let mut t = Timeline::new(
            vec![
                note(0),
                Event {
                    abs_tick: 480,
                    kind: EventKind::Tempo(1_000_000),
                },
                note(960),
            ],
            480,
            ms(0),
        );
        assert_eq!(t.total(), ms(1500));
        assert_eq!(t.elapsed(ms(250)), ms(250));
        assert_eq!(t.elapsed(ms(1250)), ms(1250));
        t.pause(ms(1250));
        assert_eq!(t.elapsed(ms(9999)), ms(1250));
    }

    #[test]
    fn focus_resume_waits_200ms_without_consuming_the_first_notes() {
        let mut t = Timeline::new(vec![note(0), note(480)], 480, ms(0));
        t.pause(ms(0));
        t.resume(FOCUS_DELAY);
        assert_eq!(t.elapsed(ms(199)), ms(0));
        assert!(t.pop_due(ms(199)).is_none());
        assert!(t.pop_due(ms(200)).is_some());
        assert_eq!(t.wait(ms(200)), Some(ms(500)));
    }

    #[test]
    fn seek_and_resume_restore_program_sustain_and_sounding_notes() {
        let mut t = Timeline::new(
            vec![
                midi(0, &[0xC2, 40]),
                midi(0, &[0xB2, 64, 127]),
                midi(0, &[0x92, 60, 90]),
                midi(120, &[0x82, 60, 0]),
                midi(240, &[0x92, 64, 100]),
                midi(960, &[0xB2, 64, 0]),
            ],
            480,
            ms(0),
        );
        t.seek(ms(500), ms(5000));
        let restored = t.restore();
        assert!(restored.contains(&vec![0xC2, 40]));
        assert!(restored.contains(&vec![0xB2, 64, 127]));
        assert!(restored.contains(&vec![0x92, 60, 90]));
        assert!(restored.contains(&vec![0x82, 60, 0]));
        assert!(restored.contains(&vec![0x92, 64, 100]));
        t.seek(ms(0), ms(6000));
        assert!(t.restore().contains(&vec![0xC2, 0]));
        assert!(!t.restore().iter().any(|b| b[0] & 0xf0 == 0x90));
    }

    #[test]
    fn parser_preserves_end_of_track_silence_and_reports_bad_files() {
        let smf = midly::Smf {
            header: midly::Header::new(
                midly::Format::SingleTrack,
                midly::Timing::Metrical(480.into()),
            ),
            tracks: vec![vec![
                midly::TrackEvent {
                    delta: 0.into(),
                    kind: midly::TrackEventKind::Midi {
                        channel: 0.into(),
                        message: midly::MidiMessage::NoteOn {
                            key: 60.into(),
                            vel: 100.into(),
                        },
                    },
                },
                midly::TrackEvent {
                    delta: 960.into(),
                    kind: midly::TrackEventKind::Meta(midly::MetaMessage::EndOfTrack),
                },
            ]],
        };
        let mut bytes = Vec::new();
        smf.write_std(&mut bytes).unwrap();
        let (events, tpb) = parse_midi_bytes(&bytes).unwrap();
        let t = Timeline::new(events, tpb, ms(0));
        assert_eq!(t.total(), ms(1000));
        assert!(parse_midi_bytes(b"invalid midi").is_err());
    }

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "miditoroblox-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn scanning_deduplicates_overlapping_roots_and_reports_missing_directories() {
        let dir = TestDir::new();
        let nested = dir.0.join("nested");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(nested.join("song.MID"), []).unwrap();
        let (files, warnings) = scan_files(
            &[
                ScanDir {
                    path: dir.0.clone(),
                    recursive: true,
                },
                ScanDir {
                    path: nested,
                    recursive: false,
                },
                ScanDir {
                    path: dir.0.join("missing"),
                    recursive: true,
                },
            ],
            || false,
        );
        assert_eq!(files.len(), 1);
        assert_eq!(warnings.len(), 1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn recursive_scan_terminates_on_symlink_cycles() {
        let dir = TestDir::new();
        std::fs::write(dir.0.join("song.mid"), []).unwrap();
        std::os::unix::fs::symlink(&dir.0, dir.0.join("loop")).unwrap();
        let (files, warnings) = scan_files(
            &[ScanDir {
                path: dir.0.clone(),
                recursive: true,
            }],
            || false,
        );
        assert_eq!(files.len(), 1);
        assert!(warnings.is_empty());
    }

    #[test]
    fn rescan_preserves_file_identity_when_indices_change() {
        let (tx, _rx) = mpsc::sync_channel(4096);
        let mut player = MidiPlayer::new(tx, Arc::new(AtomicU64::new(0)));
        player.midi_files = vec![PathBuf::from("b.mid")];
        player.selected_idx = Some(0);
        player.playing_idx = Some(0);
        player.playing_path = Some(PathBuf::from("b.mid"));
        let (notices, rx) = mpsc::channel();
        player.notices = rx;
        notices
            .send(Notice::Scanned(
                0,
                vec![PathBuf::from("a.mid"), PathBuf::from("b.mid")],
                vec![],
            ))
            .unwrap();
        player.poll();
        assert_eq!(player.selected_idx, Some(1));
        assert_eq!(player.playing_idx, Some(1));
        notices.send(Notice::Scanned(0, vec![], vec![])).unwrap();
        player.poll();
        assert_eq!(player.selected_idx, None);
        assert_eq!(player.playing_idx, None);
    }

    #[test]
    fn stop_cancels_inflight_load_results_and_invalidates_buffered_events() {
        let (tx, rx) = mpsc::sync_channel(4096);
        let epoch = Arc::new(AtomicU64::new(0));
        let mut player = MidiPlayer::new(tx, epoch.clone());
        player.loading = true;
        let stale_load_id = player.load_id.load(Ordering::SeqCst);
        let old_event = MidiEvent::at(vec![0x90, 60, 100], 0, Instant::now());
        player.stop();
        assert!(!old_event.allowed(epoch.load(Ordering::SeqCst), false, false, false));
        let (notices, notice_rx) = mpsc::channel();
        player.notices = notice_rx;
        notices
            .send(Notice::Loaded(
                stale_load_id,
                PathBuf::from("old.mid"),
                Ok((
                    Arc::new(Document {
                        hash: "test".into(),
                        instruments: vec![],
                        events: vec![note(0)],
                        owners: vec![None],
                        tpb: 480,
                    }),
                    Mutes::default(),
                    None,
                )),
            ))
            .unwrap();
        player.poll();
        assert_eq!(player.playing_idx, None);
        assert!(rx.recv_timeout(ms(250)).is_err());
    }

    #[test]
    fn playback_worker_gives_chord_notes_identical_timestamps_after_grace() {
        let (tx, rx) = mpsc::sync_channel(4096);
        let epoch = Arc::new(AtomicU64::new(1));
        let mut player = MidiPlayer::new(tx, epoch);
        let start = Instant::now();
        player.command(Cmd::Load {
            timeline: Timeline::new(
                vec![
                    midi(0, &[0x90, 60, 100]),
                    midi(0, &[0x90, 64, 100]),
                    midi(0, &[0x90, 67, 100]),
                    Event {
                        abs_tick: 960,
                        kind: EventKind::End,
                    },
                ],
                480,
                ms(0),
            ),
            epoch: 1,
        });
        let mut notes = Vec::new();
        while notes.len() < 3 {
            let event = rx.recv_timeout(Duration::from_secs(2)).unwrap();
            if event.bytes.first() == Some(&0x90) {
                notes.push(event);
            }
        }
        assert!(notes[0].received_at.duration_since(start) >= FOCUS_DELAY);
        assert_eq!(notes[0].received_at, notes[1].received_at);
        assert_eq!(notes[0].received_at, notes[2].received_at);
        player.shutdown();
    }
}
