//! Output ownership, configuration snapshots, and cancellation across UI and workers.
use crate::instrument::InstrumentSlot;
use crate::midi_event::{MidiEvent, MidiSource};
use crate::pipeline::{Cancelled, Config, Output, Pipeline};
use crate::platform::{VELOCITY_KEYS, VirtualDevice, send_key};
use crate::solver::{KeyCode, MappingGroups, jump_budget};
use eframe::egui;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
pub struct DeviceState {
    pub device: VirtualDevice,
    pub pipeline: Pipeline,
}

pub struct SharedState {
    pub shutdown: AtomicBool,
    pub focus_ready_at: Mutex<Instant>,
    pub transpose: AtomicI32,
    pub error: Mutex<Option<String>>,
    pub device_state: Mutex<DeviceState>,
    pub base_mapping_enabled: AtomicBool,
    pub low_mapping_enabled: AtomicBool,
    pub high_mapping_enabled: AtomicBool,
    pub auto_transpose_enabled: AtomicBool,
    pub experimental_transpose_enabled: AtomicBool,
    pub experimental_hold_ctrl_enabled: AtomicBool,
    pub transpose_delay_ms: AtomicU64,
    pub quantize_enabled: AtomicBool,
    pub quantize_ms: AtomicU64,
    pub solver_enabled: AtomicBool,
    pub solver_accuracy: AtomicU64,
    pub transpose_range: AtomicU64,
    pub active_notes: Mutex<std::collections::HashSet<u8>>,

    pub active_output_notes: Mutex<std::collections::HashSet<u8>>,

    pub visualizer_enabled: AtomicBool,
    pub visualizer_show_midi: AtomicBool,
    pub visualizer_show_roblox: AtomicBool,

    pub ui_context: Mutex<Option<egui::Context>>,
    pub panic_enabled: AtomicBool,
    pub panic_active: AtomicBool,
    pub panic_releases_in_flight: AtomicU64,
    pub control_velocity_enabled: AtomicBool,
    pub velocity_delay_ms: AtomicU64,
    pub sustain_enabled: AtomicBool,
    pub midi_sender: std::sync::mpsc::SyncSender<MidiEvent>,
    pub output_epoch: Arc<AtomicU64>,
    pub use_midi_device: AtomicBool,
    pub app_focused: AtomicBool,
    pub range_instrument_enabled: AtomicBool,
    pub fold_octaves: AtomicBool,
    pub instrument_slots: Mutex<[InstrumentSlot; 4]>,
}

pub fn send_live_message(shared: &SharedState, bytes: &[u8]) {
    let event = MidiEvent::new(
        MidiSource::Live,
        bytes.to_vec(),
        shared.output_epoch.load(Ordering::SeqCst),
    );
    if !shared.shutdown.load(Ordering::SeqCst)
        && event.allowed(
            shared.output_epoch.load(Ordering::SeqCst),
            shared.panic_active.load(Ordering::SeqCst),
            shared.app_focused.load(Ordering::SeqCst),
            shared.use_midi_device.load(Ordering::Relaxed),
        )
        && shared.midi_sender.try_send(event).is_err()
    {
        shared.output_epoch.fetch_add(1, Ordering::SeqCst);
        shared.report_error(
            "MIDI input queue is full or unavailable; queued output cancelled".into(),
        );
    }
}

fn output_allowed(shared: &SharedState, event: &MidiEvent) -> bool {
    !shared.shutdown.load(Ordering::SeqCst)
        && Instant::now()
            >= *shared
                .focus_ready_at
                .lock()
                .unwrap_or_else(|e| e.into_inner())
        && event.allowed(
            shared.output_epoch.load(Ordering::SeqCst),
            shared.panic_active.load(Ordering::SeqCst)
                || shared.panic_releases_in_flight.load(Ordering::SeqCst) != 0,
            shared.app_focused.load(Ordering::SeqCst),
            shared.use_midi_device.load(Ordering::Relaxed),
        )
}

fn guarded_wait(shared: &SharedState, event: &MidiEvent, duration: Duration) -> bool {
    let deadline = std::time::Instant::now() + duration;
    loop {
        if !output_allowed(shared, event) {
            return false;
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return true;
        }
        thread::sleep(remaining.min(Duration::from_millis(2)));
    }
}

struct HardwareOutput<'a> {
    shared: &'a SharedState,
    event: &'a MidiEvent,
    device: &'a mut VirtualDevice,
}

impl Output for HardwareOutput<'_> {
    fn valid(&self) -> bool {
        output_allowed(self.shared, self.event)
    }
    fn key(&mut self, key: KeyCode, down: bool) -> Result<(), Cancelled> {
        if !self.valid() {
            return Err(Cancelled);
        }
        send_key(self.device, key, down).map_err(|e| {
            // The OS may have accepted part of a failed write; force broad cleanup next.
            self.shared.output_epoch.fetch_add(1, Ordering::SeqCst);
            self.shared
                .report_error(format!("Keyboard output failed: {e}"));
            Cancelled
        })
    }
    fn release(&mut self, key: KeyCode) {
        if let Err(e) = send_key(self.device, key, false) {
            self.shared
                .report_error(format!("Could not release keyboard key: {e}"));
        }
    }

    fn wait(&mut self, duration: Duration) -> Result<(), Cancelled> {
        if guarded_wait(self.shared, self.event, duration) {
            Ok(())
        } else {
            Err(Cancelled)
        }
    }
}

pub(crate) fn config(shared: &SharedState) -> Config {
    let load = |value: &AtomicBool| value.load(Ordering::Relaxed);
    let ms = |value: &AtomicU64| Duration::from_millis(value.load(Ordering::Relaxed));
    Config {
        groups: MappingGroups {
            low: load(&shared.low_mapping_enabled),
            middle: load(&shared.base_mapping_enabled),
            high: load(&shared.high_mapping_enabled),
        },
        solver: load(&shared.solver_enabled),
        max_jump: jump_budget(shared.solver_accuracy.load(Ordering::Relaxed)),
        range: shared.transpose_range.load(Ordering::Relaxed) as i32,
        auto_octave: load(&shared.auto_transpose_enabled),
        experimental: load(&shared.experimental_transpose_enabled),
        hold_ctrl: load(&shared.experimental_hold_ctrl_enabled),
        sustain: load(&shared.sustain_enabled),
        velocity: load(&shared.control_velocity_enabled),
        routing: load(&shared.range_instrument_enabled),
        fold_octaves: load(&shared.fold_octaves),
        slots: *shared
            .instrument_slots
            .lock()
            .unwrap_or_else(|e| e.into_inner()),
        transpose_delay: ms(&shared.transpose_delay_ms),
        velocity_delay: ms(&shared.velocity_delay_ms),
    }
}

fn process_message(shared: &SharedState, event: MidiEvent) {
    let cfg = config(shared);
    let mut state = shared
        .device_state
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let DeviceState {
        device, pipeline, ..
    } = &mut *state;
    let mut output = HardwareOutput {
        shared,
        event: &event,
        device,
    };
    if !output.valid()
        && event.source == MidiSource::Live
        && shared.app_focused.load(Ordering::SeqCst)
    {
        pipeline.release_source(MidiSource::Live, &mut output);
    } else {
        let _ = pipeline.process(&event, &cfg, &mut output);
    }
    *shared
        .active_notes
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = pipeline.input_notes();
    *shared
        .active_output_notes
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = pipeline.output_notes();
    shared
        .transpose
        .store(state.pipeline.solver.current_transpose, Ordering::Relaxed);
    if let Some(ctx) = shared
        .ui_context
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
    {
        ctx.request_repaint();
    }
}

pub fn activate_panic(shared: &SharedState) {
    shared
        .panic_releases_in_flight
        .fetch_add(1, Ordering::SeqCst);
    shared.panic_active.store(true, Ordering::SeqCst);
    invalidate_output(shared);
    // Also invalidate messages produced during cleanup, even if the UI has already cleared panic.
    shared.output_epoch.fetch_add(1, Ordering::SeqCst);
    shared
        .panic_releases_in_flight
        .fetch_sub(1, Ordering::SeqCst);
}

pub fn invalidate_output(shared: &SharedState) {
    shared.output_epoch.fetch_add(1, Ordering::SeqCst);
    clear_output(shared);
}

fn clear_output(shared: &SharedState) {
    let mut state = shared
        .device_state
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    release_all_virtual_keys(&mut state, shared);
    shared
        .active_notes
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    shared
        .active_output_notes
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
}

fn release_all_virtual_keys(state: &mut DeviceState, shared_state: &SharedState) {
    let event = MidiEvent::new(
        MidiSource::Player,
        Vec::new(),
        shared_state.output_epoch.load(Ordering::SeqCst),
    );
    state.pipeline.release_all(&mut HardwareOutput {
        shared: shared_state,
        event: &event,
        device: &mut state.device,
    });
    for key in [
        KeyCode::KEY_LEFTSHIFT,
        KeyCode::KEY_LEFTCTRL,
        KeyCode::KEY_LEFTALT,
        KeyCode::KEY_UP,
        KeyCode::KEY_DOWN,
        KeyCode::KEY_APOSTROPHE,
    ]
    .into_iter()
    .chain(
        crate::solver::get_available_mappings()
            .iter()
            .map(|m| m.key_code),
    )
    .chain(VELOCITY_KEYS.into_iter().map(crate::solver::key_from_code))
    {
        if let Err(e) = send_key(&mut state.device, key, false) {
            shared_state.report_error(format!("Could not release keyboard key: {e}"));
        }
    }
}

impl SharedState {
    pub fn report_error(&self, message: String) {
        *self.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(message);
        if let Some(ctx) = self
            .ui_context
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            ctx.request_repaint();
        }
    }
}

fn return_tick(shared: &SharedState, epoch: u64) {
    let cfg = config(shared);
    let source = if shared.use_midi_device.load(Ordering::Relaxed) {
        MidiSource::Live
    } else {
        MidiSource::Player
    };
    let event = MidiEvent::new(source, Vec::new(), epoch);
    let mut state = shared
        .device_state
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let DeviceState { device, pipeline } = &mut *state;
    let mut output = HardwareOutput {
        shared,
        event: &event,
        device,
    };
    let before = pipeline.solver.current_transpose;
    let _ = pipeline.return_tick(&cfg, &mut output, Instant::now());
    shared
        .transpose
        .store(pipeline.solver.current_transpose, Ordering::Relaxed);
    if before != pipeline.solver.current_transpose
        && let Some(ctx) = shared
            .ui_context
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
    {
        ctx.request_repaint();
    }
}

pub fn spawn_output(
    shared: Arc<SharedState>,
    rx: std::sync::mpsc::Receiver<MidiEvent>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut queue = crate::schedule::EventQueue::new(
            Instant::now(),
            shared.output_epoch.load(Ordering::SeqCst),
        );
        while !shared.shutdown.load(Ordering::SeqCst) {
            if queue.transition(shared.output_epoch.load(Ordering::SeqCst)) {
                clear_output(&shared);
            }
            match rx.recv_timeout(queue.timeout(Instant::now())) {
                Ok(event) => {
                    // A command may have arrived while recv_timeout was waiting.
                    if queue.transition(shared.output_epoch.load(Ordering::SeqCst)) {
                        clear_output(&shared);
                    }
                    // Observe arrivals without looking ahead at scheduled notes.
                    if output_allowed(&shared, &event)
                        && event.bytes.len() >= 3
                        && event.bytes[0] & 0xF0 == 0x90
                        && event.bytes[1] <= 127
                        && (1..=127).contains(&event.bytes[2])
                    {
                        shared
                            .device_state
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .pipeline
                            .note_received(
                                shared.solver_enabled.load(Ordering::Relaxed),
                                Instant::now(),
                            );
                    }
                    let grid = if shared.quantize_enabled.load(Ordering::Relaxed) {
                        Duration::from_millis(shared.quantize_ms.load(Ordering::Relaxed))
                    } else {
                        Duration::ZERO
                    };
                    let ready = *shared
                        .focus_ready_at
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if queue.push(event, grid, ready).is_err() {
                        shared.output_epoch.fetch_add(1, Ordering::SeqCst);
                        shared.report_error(
                            "MIDI output backlog exceeded the limit; queued output cancelled"
                                .into(),
                        );
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
            while queue.epoch == shared.output_epoch.load(Ordering::SeqCst) {
                let Some(event) = queue.pop_due(Instant::now()) else {
                    break;
                };
                process_message(&shared, event);
            }
            return_tick(&shared, queue.epoch);
        }
        clear_output(&shared);
    })
}
