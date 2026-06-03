use eframe::egui;
use midir::{MidiInput, MidiInputConnection, MidiInputPort};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::time::{self, Duration, SystemTime, UNIX_EPOCH};

use std::thread;

#[cfg(target_os = "linux")]
use evdev::{uinput::VirtualDevice, AbsInfo, AbsoluteAxisCode, AttributeSet, Device, EventType, InputEvent, KeyCode, PropType, UinputAbsSetup};
#[cfg(target_os = "linux")]
use x11rb::connection::Connection;
#[cfg(target_os = "linux")]
use x11rb::protocol::xproto::ConnectionExt;

#[cfg(target_os = "windows")]
use solver::KeyCode;

#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy)]
pub struct InputEvent {
    pub type_: u16,
    pub code: u16,
    pub value: i32,
}

#[cfg(target_os = "windows")]
impl InputEvent {
    pub fn new(type_: u16, code: u16, value: i32) -> Self {
        Self { type_, code, value }
    }
    pub fn new_now(type_: u16, code: u16, value: i32) -> Self {
        Self { type_, code, value }
    }
}

#[cfg(target_os = "windows")]
pub struct EventType;
#[cfg(target_os = "windows")]
impl EventType {
    pub const KEY: EventTypeVal = EventTypeVal(1);
    pub const ABSOLUTE: EventTypeVal = EventTypeVal(3);
    pub const SYNCHRONIZATION: EventTypeVal = EventTypeVal(0);
}
#[cfg(target_os = "windows")]
#[derive(Copy, Clone)]
pub struct EventTypeVal(pub u16);

#[cfg(target_os = "windows")]
pub struct VirtualDevice;

#[cfg(target_os = "windows")]
impl VirtualDevice {
    pub fn emit(&self, events: &[InputEvent]) -> std::io::Result<()> {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
            SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
        };
        for ev in events {
            if ev.type_ == 1 {
                let mut input: INPUT = unsafe { std::mem::zeroed() };
                input.r#type = INPUT_KEYBOARD;
                let mut ki: KEYBDINPUT = unsafe { std::mem::zeroed() };
                ki.wVk = ev.code;
                ki.dwFlags = if ev.value == 0 { KEYEVENTF_KEYUP } else { 0 };
                input.Anonymous.ki = ki;
                unsafe {
                    SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
                }
            }
        }
        Ok(())
    }
}

mod solver;
use solver::{Solver, SolverMode};
mod midi_player;
use midi_player::{MidiPlayer, PlaybackStatus};

// Mappings are located in solver.rs

#[derive(Clone, Copy, Debug, PartialEq)]
enum InstrumentType {
    Other,
    Piano,
    Guitar,
    Bass,
    Strings,
    Brass,
    Synth,
    Drum,
}

impl InstrumentType {
    fn to_string(&self) -> &'static str {
        match self {
            InstrumentType::Other => "Other",
            InstrumentType::Piano => "Piano",
            InstrumentType::Guitar => "Guitar",
            InstrumentType::Bass => "Bass",
            InstrumentType::Strings => "Strings",
            InstrumentType::Brass => "Brass",
            InstrumentType::Synth => "Synth",
            InstrumentType::Drum => "Drum",
        }
    }
    
    fn all() -> &'static [InstrumentType] {
        &[
            InstrumentType::Other,
            InstrumentType::Piano,
            InstrumentType::Guitar,
            InstrumentType::Bass,
            InstrumentType::Strings,
            InstrumentType::Brass,
            InstrumentType::Synth,
            InstrumentType::Drum,
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct InstrumentSlot {
    enabled: bool,
    inst_type: InstrumentType,
    min_note: u8,
    max_note: u8,
    transposition: i8,
}

struct DeviceState {
    device: VirtualDevice,
    current_transpose_offset: i32,
    solver: Solver,
}

struct SharedState {
    device_state: Mutex<DeviceState>,
    base_mapping_enabled: AtomicBool,
    low_mapping_enabled: AtomicBool,
    high_mapping_enabled: AtomicBool,
    auto_transpose_enabled: AtomicBool,
    experimental_transpose_enabled: AtomicBool,
    experimental_hold_ctrl_enabled: AtomicBool,
    transpose_delay_ms: AtomicU64,
    lazy_transpose_enabled: AtomicBool,
    quantize_enabled: AtomicBool,
    quantize_ms: AtomicU64,
    solver_enabled: AtomicBool,
    solver_mode_efficiency: AtomicBool, // true = Efficiency, false = Accuracy
    solver_max_jump: AtomicU64,
    transpose_range: AtomicU64,
    active_notes: Mutex<std::collections::HashSet<u8>>,

    active_output_notes: Mutex<std::collections::HashSet<u8>>,
    
    visualizer_enabled: AtomicBool,
    visualizer_show_midi: AtomicBool,
    visualizer_show_roblox: AtomicBool,
    
    ui_context: Mutex<Option<egui::Context>>,
    panic_enabled: AtomicBool,
    panic_active: AtomicBool,
    control_velocity_enabled: AtomicBool,
    last_sent_velocity_key: AtomicU16,
    velocity_delay_ms: AtomicU64,
    sustain_enabled: AtomicBool,
    sustain_active: AtomicBool,
    drum_support_enabled: AtomicBool,
    mouse_position: Mutex<Option<(i32, i32)>>,
    piano_button_position: Mutex<Option<(i32, i32)>>,
    drum_button_position: Mutex<Option<(i32, i32)>>,
    capture_next_left_alt: AtomicU16,
    mouse_device: Mutex<Option<VirtualDevice>>,
    mouse_move_delay_ms: Mutex<f32>,
    mouse_click_hold_ms: Mutex<f32>,
    mouse_after_release_ms: Mutex<f32>,
    solo_after_switch_ms: Mutex<f32>,
    note_headstart_ms: Mutex<f32>,
    panic_cleared_at_ms: AtomicU64,
    active_solo: Mutex<Option<bool>>,
    midi_sender: std::sync::mpsc::Sender<Vec<u8>>,
    lookahead_ms: Mutex<f32>,
    use_midi_device: AtomicBool,
    app_focused: AtomicBool,
    range_instrument_enabled: AtomicBool,
    instrument_slots: Mutex<[InstrumentSlot; 4]>,
    channel_programs: Mutex<[u8; 16]>,
}
struct MidiApp {
    midi_input: Option<MidiInput>,
    available_ports: Vec<(String, MidiInputPort)>,
    selected_port_name: Option<String>,
    connection: Option<MidiInputConnection<Arc<SharedState>>>,
    shared_state: Arc<SharedState>,
    status_message: String,
    window_opacity: f32,
    always_on_top: bool,
    show_midi_window: bool,
    midi_player: MidiPlayer,
    new_dir_input: String,
    new_dir_recursive: bool,
    player_seek_pos: Option<f64>,
    player_was_panic: bool,
    player_paused_by_focus: bool,
}

impl MidiApp {
    fn new(cc: &eframe::CreationContext<'_>, virtual_device: VirtualDevice) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let midi_player = MidiPlayer::new(tx.clone());
        let mut app = Self {
            midi_input: Some(MidiInput::new("Miditoroblox Input").unwrap()),
            available_ports: Vec::new(),
            selected_port_name: None,
            connection: None,
            shared_state: Arc::new(SharedState {
                device_state: Mutex::new(DeviceState {
                    device: virtual_device,
                    current_transpose_offset: 0,
                    solver: Solver::new(),
                }),
                base_mapping_enabled: AtomicBool::new(true),
                low_mapping_enabled: AtomicBool::new(true),
                high_mapping_enabled: AtomicBool::new(true),
                auto_transpose_enabled: AtomicBool::new(true),
                experimental_transpose_enabled: AtomicBool::new(true),
                experimental_hold_ctrl_enabled: AtomicBool::new(true),
                transpose_delay_ms: AtomicU64::new(0),
                lazy_transpose_enabled: AtomicBool::new(true),
                quantize_enabled: AtomicBool::new(false),
                quantize_ms: AtomicU64::new(100),
                solver_enabled: AtomicBool::new(true),
                solver_mode_efficiency: AtomicBool::new(true),
                solver_max_jump: AtomicU64::new(3),
                transpose_range: AtomicU64::new(24),
                active_notes: Mutex::new(std::collections::HashSet::new()),
                active_output_notes: Mutex::new(std::collections::HashSet::new()),
                visualizer_enabled: AtomicBool::new(false),
                visualizer_show_midi: AtomicBool::new(true),
                visualizer_show_roblox: AtomicBool::new(true),
                ui_context: Mutex::new(None),
                panic_enabled: AtomicBool::new(true),
                panic_active: AtomicBool::new(false),
                control_velocity_enabled: AtomicBool::new(true),
                last_sent_velocity_key: AtomicU16::new(0),
                velocity_delay_ms: AtomicU64::new(0),
                sustain_enabled: AtomicBool::new(true),
                sustain_active: AtomicBool::new(true), // Assumes ON by default
                drum_support_enabled: AtomicBool::new(false),
                mouse_position: Mutex::new(None),
                piano_button_position: Mutex::new(None),
                drum_button_position: Mutex::new(None),
                capture_next_left_alt: AtomicU16::new(0),
                mouse_device: Mutex::new(build_mouse_device().ok()),
                mouse_move_delay_ms: Mutex::new(1.0),
                mouse_click_hold_ms: Mutex::new(2.0),
                mouse_after_release_ms: Mutex::new(1.0),
                solo_after_switch_ms: Mutex::new(1.0),
                note_headstart_ms: Mutex::new(3.0),
                panic_cleared_at_ms: AtomicU64::new(0),
                active_solo: Mutex::new(None),
                midi_sender: tx,
                lookahead_ms: Mutex::new(50.0),
                use_midi_device: AtomicBool::new(false),
                app_focused: AtomicBool::new(false),
                range_instrument_enabled: AtomicBool::new(false),
                instrument_slots: Mutex::new([
                    InstrumentSlot { enabled: true, inst_type: InstrumentType::Piano, min_note: 1, max_note: 88, transposition: 0 },
                    InstrumentSlot { enabled: false, inst_type: InstrumentType::Piano, min_note: 1, max_note: 88, transposition: 0 },
                    InstrumentSlot { enabled: false, inst_type: InstrumentType::Piano, min_note: 1, max_note: 88, transposition: 0 },
                    InstrumentSlot { enabled: false, inst_type: InstrumentType::Piano, min_note: 1, max_note: 88, transposition: 0 },
                ]),
                channel_programs: Mutex::new([0; 16]),
            }),
            status_message: "Ready".to_string(),
            window_opacity: 1.0,
            always_on_top: false,
            show_midi_window: false,
            midi_player,
            new_dir_input: String::new(),
            new_dir_recursive: false,
            player_seek_pos: None,
            player_was_panic: false,
            player_paused_by_focus: false,
        };

        // Spawn background worker thread
        let shared_clone = Arc::clone(&app.shared_state);
        thread::spawn(move || {
            let shared_state = shared_clone;
            let mut queue = std::collections::VecDeque::new();

            struct QueuedEvent {
                time: std::time::Instant,
                msg: Vec<u8>,
            }

            loop {
                // Clear queue immediately if Panic is active
                if shared_state.panic_active.load(Ordering::Relaxed) {
                    queue.clear();
                    while let Ok(_) = rx.try_recv() {}
                    thread::sleep(std::time::Duration::from_millis(5));
                    continue;
                }

                // 1. Drain all pending messages from channel into our local queue
                while let Ok(msg) = rx.try_recv() {
                    queue.push_back(QueuedEvent {
                        time: std::time::Instant::now(),
                        msg,
                    });
                }

                // If queue is empty, block waiting for the next message to prevent high CPU usage
                if queue.is_empty() {
                    if let Ok(msg) = rx.recv() {
                        if shared_state.panic_active.load(Ordering::Relaxed) {
                            while let Ok(_) = rx.try_recv() {}
                            continue;
                        }
                        queue.push_back(QueuedEvent {
                            time: std::time::Instant::now(),
                            msg,
                        });
                    } else {
                        break; // Channel closed
                    }
                }

                let now = std::time::Instant::now();
                let lookahead_val = *shared_state.lookahead_ms.lock().unwrap_or_else(|e| e.into_inner());
                let play_delay = std::time::Duration::from_millis(lookahead_val as u64);

                if shared_state.drum_support_enabled.load(Ordering::Relaxed) {
                    let current_solo = *shared_state.active_solo.lock().unwrap_or_else(|e| e.into_inner());

                    let mut due_drum = false;
                    let mut due_piano = false;
                    for event in &queue {
                        if now.duration_since(event.time) >= play_delay {
                            if event.msg.len() >= 3 {
                                let status = event.msg[0] & 0xF0;
                                let velocity = event.msg[2];
                                if status == 0x90 && velocity > 0 {
                                    if event.msg[0] & 0x0F == 9 { due_drum = true; } else { due_piano = true; }
                                }
                            }
                        }
                    }

                    // Lookahead: pre-switch to the first upcoming instrument when nothing is due yet
                    if !due_drum && !due_piano {
                        for event in &queue {
                            if event.msg.len() >= 3 && (event.msg[0] & 0xF0) == 0x90 && event.msg[2] > 0 {
                                let is_drum = event.msg[0] & 0x0F == 9;
                                if is_drum && current_solo != Some(false) {
                                    set_active_solo(&shared_state, Some(false));
                                } else if !is_drum && current_solo != Some(true) {
                                    set_active_solo(&shared_state, Some(true));
                                }
                                break;
                            }
                        }
                    }

                    // Collect and pop all due events so the loop below is a no-op in drum mode
                    let mut due_events: Vec<Vec<u8>> = Vec::new();
                    while let Some(event) = queue.front() {
                        if now.duration_since(event.time) >= play_delay {
                            due_events.push(queue.pop_front().unwrap().msg);
                        } else {
                            break;
                        }
                    }

                    if due_drum && due_piano {
                        // Two-pass: drum events first, then switch to piano for piano events
                        let curr = *shared_state.active_solo.lock().unwrap_or_else(|e| e.into_inner());
                        if curr != Some(false) {
                            set_active_solo(&shared_state, Some(false));
                        }
                        for msg in due_events.iter().filter(|m| !m.is_empty() && (m[0] & 0x0F) == 9) {
                            process_message(&shared_state, msg.clone());
                        }
                        set_active_solo(&shared_state, Some(true));
                        for msg in due_events.iter().filter(|m| !m.is_empty() && (m[0] & 0x0F) != 9) {
                            process_message(&shared_state, msg.clone());
                        }
                    } else {
                        let curr = *shared_state.active_solo.lock().unwrap_or_else(|e| e.into_inner());
                        if due_drum && curr != Some(false) {
                            set_active_solo(&shared_state, Some(false));
                        } else if due_piano && curr != Some(true) {
                            set_active_solo(&shared_state, Some(true));
                        }
                        for msg in due_events {
                            process_message(&shared_state, msg);
                        }
                    }
                }

                // Play all events that are due
                while let Some(event) = queue.front() {
                    if now.duration_since(event.time) >= play_delay {
                        let event = queue.pop_front().unwrap();
                        let message = event.msg;

                         if shared_state.panic_active.load(Ordering::Relaxed) { continue; }
                         if shared_state.use_midi_device.load(Ordering::Relaxed) && shared_state.app_focused.load(Ordering::Relaxed) {
                             continue;
                         }
                        if message.len() < 2 { continue; }
                        let status = message[0] & 0xF0;
                        if status == 0xC0 {
                            let ch = (message[0] & 0x0F) as usize;
                            if ch < 16 {
                                if let Ok(mut cp) = shared_state.channel_programs.lock() {
                                    cp[ch] = message[1];
                                }
                            }
                            continue;
                        }
                        if message.len() < 3 { continue; }
                        let channel = message[0] & 0x0F;
                        if channel == 9 
                            && !shared_state.drum_support_enabled.load(Ordering::Relaxed)
                            && !shared_state.range_instrument_enabled.load(Ordering::Relaxed) 
                        {
                            continue;
                        }
                        let note_original = message[1];
                        let velocity = message[2];

                        let mut targets = Vec::new();
                        let is_note_event = status == 0x90 || status == 0x80;
                        if is_note_event && shared_state.range_instrument_enabled.load(Ordering::Relaxed) {
                            let program = if let Ok(cp) = shared_state.channel_programs.lock() {
                                cp[channel as usize]
                            } else {
                                0
                            };
                            let is_drum_channel = channel == 9;
                            if let Ok(slots) = shared_state.instrument_slots.lock() {
                                // First pass: check if any specific (non-Other) enabled slot claims this note
                                let mut claimed_by_specific = false;
                                for slot in slots.iter() {
                                    if slot.enabled && slot.inst_type != InstrumentType::Other
                                        && match_instrument(channel, program, slot.inst_type)
                                    {
                                        let note_1_88 = note_original as i32 - 20;
                                        let physical = note_1_88 - slot.transposition as i32;
                                        if physical >= slot.min_note as i32 && physical <= slot.max_note as i32 {
                                            let target_midi = physical + 20;
                                            targets.push(target_midi as u8);
                                            claimed_by_specific = true;
                                        }
                                    }
                                }
                                // Second pass: route to Other if unclaimed (never drums)
                                if !claimed_by_specific && !is_drum_channel {
                                    for slot in slots.iter() {
                                        if slot.enabled && slot.inst_type == InstrumentType::Other {
                                            let note_1_88 = note_original as i32 - 20;
                                            let physical = note_1_88 - slot.transposition as i32;
                                            if physical >= slot.min_note as i32 && physical <= slot.max_note as i32 {
                                                let target_midi = physical + 20;
                                                targets.push(target_midi as u8);
                                            }
                                            break; // only one Other slot
                                        }
                                    }
                                }
                            }
                        } else {
                            targets.push(note_original);
                        }

                        for note_original in targets {

                        // Visualizer Update
                        if status == 0x90 && velocity > 0 {
                            if let Ok(mut notes) = shared_state.active_notes.lock() { notes.insert(note_original); }
                            if let Ok(ctx_opt) = shared_state.ui_context.lock() { if let Some(ctx) = ctx_opt.as_ref() { ctx.request_repaint(); } }
                        } else if status == 0x80 || (status == 0x90 && velocity == 0) {
                            if let Ok(mut notes) = shared_state.active_notes.lock() { notes.remove(&note_original); }
                            if let Ok(ctx_opt) = shared_state.ui_context.lock() { if let Some(ctx) = ctx_opt.as_ref() { ctx.request_repaint(); } }
                        }

                        // Sustain Pedal Control
                        if status == 0xB0 && shared_state.sustain_enabled.load(Ordering::Relaxed) && message[1] == 64 {
                            let pedal_down = message[2] >= 64;
                            if pedal_down != shared_state.sustain_active.load(Ordering::Relaxed) {
                                if let Ok(mut state) = shared_state.device_state.lock() {
                                    let _ = state.device.emit(&[
                                        InputEvent::new(EventType::KEY.0, KeyCode::KEY_APOSTROPHE.code(), 1),
                                        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                    ]);
                                    let _ = state.device.emit(&[
                                        InputEvent::new(EventType::KEY.0, KeyCode::KEY_APOSTROPHE.code(), 0),
                                        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                    ]);
                                    shared_state.sustain_active.store(pedal_down, Ordering::Relaxed);
                                }
                            }
                            continue;
                        }

                        let is_note_valid = |n: u8| -> bool {
                            if n < 36 { shared_state.low_mapping_enabled.load(Ordering::Relaxed) }
                            else if n > 96 { shared_state.high_mapping_enabled.load(Ordering::Relaxed) }
                            else { shared_state.base_mapping_enabled.load(Ordering::Relaxed) }
                        };

                        let handle_velocity = |state: &mut DeviceState, v: u8, shared: &SharedState| {
                            if v > 0 && shared.control_velocity_enabled.load(Ordering::Relaxed) {
                                let target_key = VELOCITY_KEYS[((v as u32 * 31) / 127) as usize];
                                if target_key != shared.last_sent_velocity_key.load(Ordering::Relaxed) {
                                    let delay = shared.velocity_delay_ms.load(Ordering::Relaxed);
                                    let _ = state.device.emit(&[
                                        InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTALT.code(), 1),
                                        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                    ]);
                                    if delay > 0 { thread::sleep(time::Duration::from_millis(delay)); }
                                    let _ = state.device.emit(&[
                                        InputEvent::new(EventType::KEY.0, target_key, 1),
                                        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                    ]);
                                    if delay > 0 { thread::sleep(time::Duration::from_millis(delay)); }
                                    let _ = state.device.emit(&[
                                        InputEvent::new(EventType::KEY.0, target_key, 0),
                                        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                    ]);
                                    if delay > 0 { thread::sleep(time::Duration::from_millis(delay)); }
                                    let _ = state.device.emit(&[
                                        InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTALT.code(), 0),
                                        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                    ]);
                                    shared.last_sent_velocity_key.store(target_key, Ordering::Relaxed);
                                }
                            }
                        };

                        let (mut final_note, mut valid) = (note_original, is_note_valid(note_original));
                        let use_solver = shared_state.solver_enabled.load(Ordering::Relaxed);

                        if !use_solver {
                            if !valid && shared_state.auto_transpose_enabled.load(Ordering::Relaxed) {
                                let mut tn = final_note;
                                while tn <= 108 && !is_note_valid(tn) { if let Some(n) = tn.checked_add(12) { tn = n; } else { break; } }
                                if is_note_valid(tn) { final_note = tn; valid = true; } 
                                else {
                                    tn = final_note;
                                    while tn >= 21 && !is_note_valid(tn) { if let Some(n) = tn.checked_sub(12) { tn = n; } else { break; } }
                                    if is_note_valid(tn) { final_note = tn; valid = true; }
                                }
                            }
                            if !valid { continue; }
                        }

                        if status == 0x90 && velocity > 0 && shared_state.quantize_enabled.load(Ordering::Relaxed) {
                            let grid = shared_state.quantize_ms.load(Ordering::Relaxed);
                            if grid > 0 { if let Ok(d) = SystemTime::now().duration_since(UNIX_EPOCH) { let rem = (d.as_millis() as u64) % grid; if rem > 0 { thread::sleep(time::Duration::from_millis(grid - rem)); } } }
                        }

                        if use_solver {
                            let mut state = shared_state.device_state.lock().unwrap();
                            if status == 0x90 && velocity > 0 {
                                let note_headstart_ms = *shared_state.note_headstart_ms.lock().unwrap_or_else(|e| e.into_inner());
                                if note_headstart_ms > 0.0 {
                                    thread::sleep(Duration::from_secs_f32(note_headstart_ms / 1000.0));
                                }
                                handle_velocity(&mut state, velocity, &shared_state);
                                let (mode, max_j, range) = (if shared_state.solver_mode_efficiency.load(Ordering::Relaxed) { SolverMode::Efficiency } else { SolverMode::Accuracy }, shared_state.solver_max_jump.load(Ordering::Relaxed) as i32, shared_state.transpose_range.load(Ordering::Relaxed) as i32);
                                let (effective_min, effective_max) = if shared_state.range_instrument_enabled.load(Ordering::Relaxed) {
                                    if let Ok(slots) = shared_state.instrument_slots.lock() {
                                        let mut lo = -range;
                                        let mut hi = range;
                                        for slot in slots.iter() {
                                            if slot.enabled && slot.inst_type != InstrumentType::Drum {
                                                // slot.transposition is the game's per-instrument setting.
                                                // global + slot.transposition must stay within [-24, 24].
                                                lo = lo.max(-24 - slot.transposition as i32);
                                                hi = hi.min(24 - slot.transposition as i32);
                                            }
                                        }
                                        (lo, hi)
                                    } else {
                                        (-range, range)
                                    }
                                } else {
                                    (-range, range)
                                };
                                if let Some((delta, m)) = state.solver.solve_bounded(note_original, mode, max_j, effective_min, effective_max) {
                                    if let Ok(mut out) = shared_state.active_output_notes.lock() { out.insert(note_original); }
                                    let curr = state.solver.current_transpose;
                                    if delta != curr {
                                        let diff = delta - curr;
                                        let key = if diff > 0 { KeyCode::KEY_UP } else { KeyCode::KEY_DOWN };
                                        for _ in 0..diff.abs() {
                                            let _ = state.device.emit(&[
                                                InputEvent::new(EventType::KEY.0, key.code(), 1),
                                                InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                            ]);
                                            let _ = state.device.emit(&[
                                                InputEvent::new(EventType::KEY.0, key.code(), 0),
                                                InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                            ]);
                                            thread::sleep(time::Duration::from_millis(5));
                                        }
                                        state.current_transpose_offset = delta;
                                    }
                                    if state.solver.active_keys.contains_key(&m.key_code) {
                                        let _ = state.device.emit(&[
                                            InputEvent::new(EventType::KEY.0, m.key_code.code(), 0),
                                            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                        ]);
                                        thread::sleep(time::Duration::from_millis(5));
                                    }
                                    if m.shift && !state.solver.shift_active {
                                        let _ = state.device.emit(&[
                                            InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 1),
                                            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                        ]);
                                    } else if !m.shift && state.solver.shift_active {
                                        let _ = state.device.emit(&[
                                            InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 0),
                                            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                        ]);
                                    }
                                    if m.ctrl && !state.solver.ctrl_active {
                                        let _ = state.device.emit(&[
                                            InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 1),
                                            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                        ]);
                                    } else if !m.ctrl && state.solver.ctrl_active {
                                        let _ = state.device.emit(&[
                                            InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 0),
                                            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                        ]);
                                    }
                                    let _ = state.device.emit(&[
                                        InputEvent::new(EventType::KEY.0, m.key_code.code(), 1),
                                        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                    ]);
                                    state.solver.register_note_on(m.key_code, note_original, delta, m.shift, m.ctrl);
                                }
                            } else {
                                if let Some(key) = state.solver.register_note_off(note_original) {
                                    if let Ok(mut out) = shared_state.active_output_notes.lock() { out.remove(&note_original); }
                                    let _ = state.device.emit(&[
                                        InputEvent::new(EventType::KEY.0, key.code(), 0),
                                        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                    ]);
                                    if !state.solver.shift_active {
                                        let _ = state.device.emit(&[
                                            InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 0),
                                            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                        ]);
                                    }
                                    if !state.solver.ctrl_active {
                                        let _ = state.device.emit(&[
                                            InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 0),
                                            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                        ]);
                                    }
                                }
                            }
                            continue;
                        }

                        let experimental = shared_state.experimental_transpose_enabled.load(Ordering::Relaxed);
                        let hold_ctrl = shared_state.experimental_hold_ctrl_enabled.load(Ordering::Relaxed);
                        let ms = solver::get_available_mappings();
                        if let Some(m) = ms.iter().find(|map| map.midi_note == final_note) {
                            let mut state = shared_state.device_state.lock().unwrap();
                            if status == 0x90 && velocity > 0 {
                                handle_velocity(&mut state, velocity, &shared_state);
                                if let Ok(mut out) = shared_state.active_output_notes.lock() { out.insert(note_original); }
                                let mut handled_t = false;
                                if experimental {
                                    if shared_state.lazy_transpose_enabled.load(Ordering::Relaxed) {
                                        let (target, current) = (if m.shift && !m.ctrl { 1 } else { 0 }, state.current_transpose_offset);
                                        if target != current {
                                            let d = shared_state.transpose_delay_ms.load(Ordering::Relaxed);
                                            let k = if target > current { KeyCode::KEY_UP } else { KeyCode::KEY_DOWN };
                                            let _ = state.device.emit(&[
                                                InputEvent::new(EventType::KEY.0, k.code(), 1),
                                                InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                            ]);
                                            let _ = state.device.emit(&[
                                                InputEvent::new(EventType::KEY.0, k.code(), 0),
                                                InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                            ]);
                                            if d > 0 { drop(state); thread::sleep(time::Duration::from_millis(d)); state = shared_state.device_state.lock().unwrap(); }
                                            state.current_transpose_offset = target;
                                        }
                                        handled_t = true;
                                    } else { state.current_transpose_offset = 0; }
                                }
                                if m.ctrl {
                                    if hold_ctrl {
                                        let _ = state.device.emit(&[
                                            InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 1),
                                            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                        ]);
                                        let _ = state.device.emit(&[
                                            InputEvent::new(EventType::KEY.0, m.key_code.code(), 1),
                                            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                        ]);
                                        let _ = state.device.emit(&[
                                            InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 0),
                                            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                        ]);
                                    } else {
                                        let _ = state.device.emit(&[
                                            InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 1),
                                            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                        ]);
                                        let _ = state.device.emit(&[
                                            InputEvent::new(EventType::KEY.0, m.key_code.code(), 1),
                                            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                        ]);
                                        let _ = state.device.emit(&[
                                            InputEvent::new(EventType::KEY.0, m.key_code.code(), 0),
                                            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                        ]);
                                        let _ = state.device.emit(&[
                                            InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 0),
                                            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                        ]);
                                    }
                                } else if m.shift {
                                    if experimental {
                                        if handled_t {
                                            let _ = state.device.emit(&[
                                                InputEvent::new(EventType::KEY.0, m.key_code.code(), 1),
                                                InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                            ]);
                                        } else {
                                            let d = shared_state.transpose_delay_ms.load(Ordering::Relaxed);
                                            let _ = state.device.emit(&[
                                                InputEvent::new(EventType::KEY.0, KeyCode::KEY_UP.code(), 1),
                                                InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                            ]);
                                            let _ = state.device.emit(&[
                                                InputEvent::new(EventType::KEY.0, KeyCode::KEY_UP.code(), 0),
                                                InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                            ]);
                                            if d > 0 { drop(state); thread::sleep(time::Duration::from_millis(d)); state = shared_state.device_state.lock().unwrap(); }
                                            let _ = state.device.emit(&[
                                                InputEvent::new(EventType::KEY.0, m.key_code.code(), 1),
                                                InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                            ]);
                                            if d > 0 { drop(state); thread::sleep(time::Duration::from_millis(d)); state = shared_state.device_state.lock().unwrap(); }
                                            let _ = state.device.emit(&[
                                                InputEvent::new(EventType::KEY.0, KeyCode::KEY_DOWN.code(), 1),
                                                InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                            ]);
                                            let _ = state.device.emit(&[
                                                InputEvent::new(EventType::KEY.0, KeyCode::KEY_DOWN.code(), 0),
                                                InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                            ]);
                                        }
                                    } else {
                                        let _ = state.device.emit(&[
                                            InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 1),
                                            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                        ]);
                                        let _ = state.device.emit(&[
                                            InputEvent::new(EventType::KEY.0, m.key_code.code(), 1),
                                            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                        ]);
                                        let _ = state.device.emit(&[
                                            InputEvent::new(EventType::KEY.0, m.key_code.code(), 0),
                                            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                        ]);
                                        let _ = state.device.emit(&[
                                            InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 0),
                                            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                        ]);
                                    }
                                } else {
                                    let _ = state.device.emit(&[
                                        InputEvent::new(EventType::KEY.0, m.key_code.code(), 1),
                                        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                    ]);
                                }
                            } else {
                                if let Ok(mut out) = shared_state.active_output_notes.lock() { out.remove(&note_original); }
                                if m.ctrl && hold_ctrl {
                                    let _ = state.device.emit(&[
                                        InputEvent::new(EventType::KEY.0, m.key_code.code(), 0),
                                        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                    ]);
                                } else if m.shift && experimental {
                                    let _ = state.device.emit(&[
                                        InputEvent::new(EventType::KEY.0, m.key_code.code(), 0),
                                        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                    ]);
                                } else if !m.shift && !m.ctrl {
                                    let _ = state.device.emit(&[
                                        InputEvent::new(EventType::KEY.0, m.key_code.code(), 0),
                                        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                                    ]);
                                }
                            }
                        }
                        }
                    } else {
                        break; // Not due yet
                    }
                }

                thread::sleep(std::time::Duration::from_millis(1));
            }
        });

        // Initialize visuals (opaque default)
        let mut visuals = egui::Visuals::dark();
        visuals.window_fill = egui::Color32::from_black_alpha(255);
        visuals.panel_fill = egui::Color32::from_black_alpha(255);
        cc.egui_ctx.set_visuals(visuals);

        app.refresh_ports();
        app.spawn_panic_monitor();
        app.spawn_mouse_monitor();
        app.spawn_instrument_keepalive();
        app
    }

    #[cfg(target_os = "linux")]
    fn spawn_panic_monitor(&self) {
        let shared = self.shared_state.clone();
        thread::spawn(move || {
            let dir = match std::fs::read_dir("/dev/input") {
                Ok(d) => d,
                Err(_) => return,
            };

            for entry in dir.flatten() {
                let path = entry.path();
                if path.file_name().and_then(|n| n.to_str()).map_or(false, |s| s.starts_with("event")) {
                    let shared_inner = shared.clone();
                    let path_inner = path.clone();
                    thread::spawn(move || {
                        let mut device = match Device::open(&path_inner) {
                            Ok(d) => d,
                            Err(_) => return,
                        };

                        // Check if it has LEFTALT or RIGHTALT
                        let supported = device.supported_keys();
                        if !supported.map_or(false, |keys| keys.contains(KeyCode::KEY_LEFTALT) || keys.contains(KeyCode::KEY_RIGHTALT)) {
                            return;
                        }

                        let mut right_alt_down = false;

                        loop {
                            match device.fetch_events() {
                                Ok(events) => {
                                    for ev in events {
                                        if ev.event_type() == EventType::KEY {
                                            let code = ev.code();
                                            let value = ev.value();
                                            
                                            // Right ALT Panic
                                            if code == KeyCode::KEY_RIGHTALT.code() && value == 1 {
                                                if shared_inner.panic_enabled.load(Ordering::Relaxed) {
                                                    shared_inner.panic_active.store(true, Ordering::Relaxed);
                                                    
                                                    if let Ok(mut state) = shared_inner.device_state.lock() {
                                                        release_all_virtual_keys(&mut state, &shared_inner);
                                                    }
                                                     if let Ok(mut notes) = shared_inner.active_notes.lock() { notes.clear(); }
                                                     if let Ok(mut notes) = shared_inner.active_output_notes.lock() { notes.clear(); }

                                                    if let Ok(ctx_opt) = shared_inner.ui_context.lock() {
                                                        if let Some(ctx) = ctx_opt.as_ref() {
                                                            ctx.request_repaint();
                                                        }
                                                    }
                                                }
                                            }

                                            if code == KeyCode::KEY_LEFTALT.code() && value == 1 && shared_inner.drum_support_enabled.load(Ordering::Relaxed) {
                                                let stage = shared_inner.capture_next_left_alt.load(Ordering::Relaxed);
                                                if stage > 0 {
                                                    let pos = shared_inner.mouse_position.lock().ok().and_then(|p| *p);
                                                    if let Some((x, y)) = pos {
                                                        if stage == 1 {
                                                            if let Ok(mut p) = shared_inner.piano_button_position.lock() {
                                                                *p = Some((x, y));
                                                            }
                                                            shared_inner.capture_next_left_alt.store(2, Ordering::Relaxed);
                                                        } else if stage == 2 {
                                                            if let Ok(mut p) = shared_inner.drum_button_position.lock() {
                                                                *p = Some((x, y));
                                                            }
                                                            shared_inner.capture_next_left_alt.store(0, Ordering::Relaxed);
                                                        }
                                                    }
                                                }
                                            }

                                            // Right ALT Velocity
                                            if code == KeyCode::KEY_RIGHTALT.code() {
                                                right_alt_down = value != 0;
                                            }

                                            if right_alt_down && value == 1 && shared_inner.control_velocity_enabled.load(Ordering::Relaxed) {
                                                if is_velocity_key(code) {
                                                    if let Ok(mut state) = shared_inner.device_state.lock() {
                                                        let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, code, 1)]);
                                                        let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, code, 0)]);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                    });
                }
            }
        });
    }

    #[cfg(target_os = "windows")]
    fn spawn_panic_monitor(&self) {
        let shared = self.shared_state.clone();
        thread::spawn(move || {
            use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
                VK_RMENU, VK_LMENU,
            };
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                SetWindowsHookExW, CallNextHookEx, UnhookWindowsHookEx,
                WH_KEYBOARD_LL, GetMessageW, MSG, HC_ACTION, WM_KEYDOWN,
                WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP, KBDLLHOOKSTRUCT,
            };

            thread_local! {
                static SHARED_STATE: std::cell::RefCell<Option<Arc<SharedState>>> = std::cell::RefCell::new(None);
                static RIGHT_ALT_DOWN: std::cell::Cell<bool> = std::cell::Cell::new(false);
            }

            SHARED_STATE.with(|s| *s.borrow_mut() = Some(shared.clone()));

            unsafe extern "system" fn hook_proc(code: i32, wparam: usize, lparam: isize) -> isize {
                if code == HC_ACTION as i32 {
                    let hook_struct = *(lparam as *const KBDLLHOOKSTRUCT);
                    let vk = hook_struct.vkCode as u16;
                    let is_down = wparam == WM_KEYDOWN as usize || wparam == WM_SYSKEYDOWN as usize;
                    
                    SHARED_STATE.with(|s| {
                        if let Some(shared_inner) = s.borrow().as_ref() {
                            // 1. Right ALT Panic
                            if vk == VK_RMENU && is_down {
                                if shared_inner.panic_enabled.load(Ordering::Relaxed) {
                                    shared_inner.panic_active.store(true, Ordering::Relaxed);
                                    if let Ok(mut state) = shared_inner.device_state.lock() {
                                        release_all_virtual_keys(&mut state, &shared_inner);
                                    }
                                    if let Ok(mut notes) = shared_inner.active_notes.lock() { notes.clear(); }
                                    if let Ok(mut notes) = shared_inner.active_output_notes.lock() { notes.clear(); }
                                    if let Ok(ctx_opt) = shared_inner.ui_context.lock() {
                                        if let Some(ctx) = ctx_opt.as_ref() {
                                            ctx.request_repaint();
                                        }
                                    }
                                }
                            }

                            // 2. Left ALT Capture
                            if vk == VK_LMENU && is_down && shared_inner.drum_support_enabled.load(Ordering::Relaxed) {
                                let stage = shared_inner.capture_next_left_alt.load(Ordering::Relaxed);
                                if stage > 0 {
                                    let pos = shared_inner.mouse_position.lock().ok().and_then(|p| *p);
                                    if let Some((x, y)) = pos {
                                        if stage == 1 {
                                            if let Ok(mut p) = shared_inner.piano_button_position.lock() {
                                                *p = Some((x, y));
                                            }
                                            shared_inner.capture_next_left_alt.store(2, Ordering::Relaxed);
                                        } else if stage == 2 {
                                            if let Ok(mut p) = shared_inner.drum_button_position.lock() {
                                                *p = Some((x, y));
                                            }
                                            shared_inner.capture_next_left_alt.store(0, Ordering::Relaxed);
                                        }
                                    }
                                }
                            }

                            // 3. Right ALT Velocity
                            if vk == VK_RMENU {
                                RIGHT_ALT_DOWN.with(|r| r.set(is_down));
                            }

                            if RIGHT_ALT_DOWN.with(|r| r.get()) && is_down && shared_inner.control_velocity_enabled.load(Ordering::Relaxed) {
                                if is_velocity_key(vk) {
                                    if let Ok(mut state) = shared_inner.device_state.lock() {
                                        let _ = state.device.emit(&[InputEvent::new(1, vk, 1)]);
                                        let _ = state.device.emit(&[InputEvent::new(1, vk, 0)]);
                                    }
                                }
                            }
                        }
                    });
                }
                CallNextHookEx(0, code, wparam, lparam)
            }

            unsafe {
                let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), 0, 0);
                if hook != 0 {
                    let mut msg: MSG = std::mem::zeroed();
                    while GetMessageW(&mut msg, 0, 0, 0) != 0 {
                        // Standard message loop
                    }
                    UnhookWindowsHookEx(hook);
                }
            }
        });
    }

    #[cfg(target_os = "linux")]
    fn spawn_mouse_monitor(&self) {
        let shared = self.shared_state.clone();
        thread::spawn(move || {
            let (conn, screen_num) = match x11rb::connect(None) {
                Ok(v) => v,
                Err(_) => return,
            };
            let root = conn.setup().roots[screen_num].root;

            loop {
                if let Ok(cookie) = conn.query_pointer(root) {
                    if let Ok(reply) = cookie.reply() {
                        if let Ok(mut pos) = shared.mouse_position.lock() {
                            *pos = Some((i32::from(reply.root_x), i32::from(reply.root_y)));
                        }
                    }
                }
                thread::sleep(time::Duration::from_millis(8));
            }
        });
    }

    #[cfg(target_os = "windows")]
    fn spawn_mouse_monitor(&self) {
        let shared = self.shared_state.clone();
        thread::spawn(move || {
            use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
            use windows_sys::Win32::Foundation::POINT;
            loop {
                let mut pt: POINT = unsafe { std::mem::zeroed() };
                if unsafe { GetCursorPos(&mut pt) } != 0 {
                    if let Ok(mut pos) = shared.mouse_position.lock() {
                        *pos = Some((pt.x, pt.y));
                    }
                }
                thread::sleep(time::Duration::from_millis(8));
            }
        });
    }

    fn spawn_instrument_keepalive(&self) {
        let shared = self.shared_state.clone();
        thread::spawn(move || {
            loop {
                if shared.panic_active.load(Ordering::Relaxed) {
                    shared.panic_cleared_at_ms.store(current_millis(), Ordering::Relaxed);
                    if let Ok(mut active) = shared.active_solo.lock() {
                        *active = None;
                    }
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }

                let cleared_at = shared.panic_cleared_at_ms.load(Ordering::Relaxed);
                if cleared_at != 0 {
                    let elapsed = current_millis().saturating_sub(cleared_at);
                    if elapsed < 2000 {
                        thread::sleep(Duration::from_millis(100));
                        continue;
                    }
                    shared.panic_cleared_at_ms.store(0, Ordering::Relaxed);
                }

                thread::sleep(Duration::from_millis(100));
            }
        });
    }

    fn refresh_ports(&mut self) {
        if self.connection.is_some() {
            return;
        }

        let midi_in = match &self.midi_input {
            Some(m) => m,
            None => {
                // If we don't have one (shouldn't happen unless we failed to create it earlier), try to create one
                 match MidiInput::new("Miditoroblox Input") {
                     Ok(m) => {
                         self.midi_input = Some(m);
                         self.midi_input.as_ref().unwrap()
                     },
                     Err(e) => {
                         self.status_message = format!("Failed to create MidiInput: {}", e);
                         return;
                     }
                 }
            }
        };

        self.available_ports.clear();
        for port in midi_in.ports() {
            let name = midi_in.port_name(&port).unwrap_or_else(|_| "Unknown".to_string());
            self.available_ports.push((name, port));
        }
        
        // Reset selection if invalid
        if let Some(selected) = &self.selected_port_name {
            if !self.available_ports.iter().any(|(n, _)| n == selected) {
                self.selected_port_name = None;
            }
        }
        
        // Auto-select first if none selected and ports exist
        if self.selected_port_name.is_none() && !self.available_ports.is_empty() {
             self.selected_port_name = Some(self.available_ports[0].0.clone());
        }
    }
}

impl eframe::App for MidiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Store context for background threads to request repaint
        if let Ok(mut c) = self.shared_state.ui_context.lock() {
            *c = Some(ctx.clone());
        }

        let is_focused = ctx.input(|i| i.focused);
        self.shared_state.app_focused.store(is_focused, Ordering::Relaxed);

        let use_midi = self.shared_state.use_midi_device.load(Ordering::Relaxed);

        if !use_midi {
            let status = self.midi_player.status();
            if is_focused {
                if status == PlaybackStatus::Playing {
                    self.midi_player.pause();
                    self.player_paused_by_focus = true;
                    self.status_message = "Paused: Focus Roblox to continue playing".to_string();
                }
            } else {
                if self.player_paused_by_focus && status == PlaybackStatus::Paused {
                    self.midi_player.resume();
                    self.player_paused_by_focus = false;
                    self.status_message = "Playing".to_string();
                }
            }
        }

        // Check panic state transitions
        let is_panic = self.shared_state.panic_active.load(Ordering::Relaxed);
        if is_panic {
            if !use_midi {
                self.midi_player.pause();
                self.player_paused_by_focus = false;
                self.shared_state.panic_active.store(false, Ordering::Relaxed);
            } else if !self.player_was_panic {
                self.midi_player.pause();
                self.player_was_panic = true;
            }
        } else {
            self.player_was_panic = false;
        }

        if self.midi_player.status() == PlaybackStatus::Playing {
            ctx.request_repaint();
        }

        // Top panel
        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Miditoroblox");
                ui.add_space(20.0);
                
                // Left Side
                ui.horizontal(|ui| {
                    let mut use_midi = self.shared_state.use_midi_device.load(Ordering::Relaxed);
                    if ui.checkbox(&mut use_midi, "MIDI Device").changed() {
                        self.shared_state.use_midi_device.store(use_midi, Ordering::Relaxed);
                        if use_midi {
                            self.midi_player.stop();
                            self.player_paused_by_focus = false;
                            if self.midi_input.is_none() {
                                if let Ok(m) = MidiInput::new("Miditoroblox Input") {
                                    self.midi_input = Some(m);
                                }
                            }
                            self.refresh_ports();
                            if let Some(port_name) = &self.selected_port_name {
                                if let Some((_, port)) = self.available_ports.iter().find(|(n, _)| n == port_name) {
                                    if let Some(midi_in) = self.midi_input.take() {
                                        let shared_clone = self.shared_state.clone();
                                        match midi_in.connect(port, "miditoroblox-in", move |_stamp, message, shared_state| {
                                            let _ = shared_state.midi_sender.send(message.to_vec());
                                        }, shared_clone) {
                                            Ok(conn) => {
                                                self.connection = Some(conn);
                                                self.status_message = format!("Connected to {}", port_name);
                                            }
                                            Err(e) => {
                                                self.status_message = format!("Error connecting: {}", e);
                                                self.midi_input = Some(e.into_inner());
                                            }
                                        }
                                    }
                                }
                            }
                        } else {
                            self.connection = None;
                            if self.midi_input.is_none() {
                                if let Ok(m) = MidiInput::new("Miditoroblox Input") {
                                    self.midi_input = Some(m);
                                }
                            }
                            self.status_message = "Midi device disabled".to_string();
                        }
                    }
                    
                    if use_midi {
                         let selected_text = self.selected_port_name.clone().unwrap_or_else(|| "Select Port...".to_string());
                         egui::ComboBox::new("midi_selector_header", "")
                             .selected_text(selected_text)
                             .show_ui(ui, |ui| {
                                 for (name, _) in &self.available_ports {
                                     if ui.selectable_label(self.selected_port_name.as_ref() == Some(name), name).clicked() {
                                         self.selected_port_name = Some(name.clone());
                                     }
                                 }
                             });
                         if ui.button("⟲").on_hover_text("Refresh MIDI ports").clicked() {
                             self.refresh_ports();
                         }
                    }
                });

                // Right Side
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.checkbox(&mut self.always_on_top, "Always On Top").changed() {
                        let level = if self.always_on_top { egui::WindowLevel::AlwaysOnTop } else { egui::WindowLevel::Normal };
                        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
                    }
                    ui.add_space(10.0);
                    ui.label("Opacity:");
                    if ui.add(egui::Slider::new(&mut self.window_opacity, 0.1..=1.0).show_value(false)).changed() {
                        let mut regulars = egui::Visuals::dark();
                        let alpha = (self.window_opacity * 255.0) as u8;
                        regulars.window_fill = egui::Color32::from_black_alpha(alpha);
                        regulars.panel_fill = egui::Color32::from_black_alpha(alpha);
                        ctx.set_visuals(regulars);
                    }
                    ui.add_space(10.0);
                    let mut panic_en = self.shared_state.panic_enabled.load(Ordering::Relaxed);
                    if ui.checkbox(&mut panic_en, "ALT to Panic").on_hover_text("Right ALT to Panic").changed() {
                        self.shared_state.panic_enabled.store(panic_en, Ordering::Relaxed);
                    }
                });
            });
        });

        // Bottom panels (Order: absolute bottom first, then stacked on it)
        egui::TopBottomPanel::bottom("footer").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let transpose = if let Ok(state) = self.shared_state.device_state.lock() { state.current_transpose_offset } else { 0 };
                let active_solo_str = match *self.shared_state.active_solo.lock().unwrap_or_else(|e| e.into_inner()) {
                    Some(true) => "Piano",
                    Some(false) => "Drum",
                    None => "None (Both Off)",
                };
                ui.label(egui::RichText::new(format!("In-Game Transpose: {}  |  Active Solo: {}", transpose, active_solo_str)).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Reset All").clicked() {
                        if let Ok(mut state) = self.shared_state.device_state.lock() {
                            state.solver.reset_transpose();
                            state.current_transpose_offset = 0;
                            release_all_virtual_keys(&mut state, &self.shared_state);
                        }
                        if let Ok(mut notes) = self.shared_state.active_notes.lock() { notes.clear(); }
                        if let Ok(mut notes) = self.shared_state.active_output_notes.lock() { notes.clear(); }
                    }
                    ui.add_space(10.0);
                    if ui.button("Release All").clicked() {
                        if let Ok(mut state) = self.shared_state.device_state.lock() {
                            release_all_virtual_keys(&mut state, &self.shared_state);
                        }
                        if let Ok(mut notes) = self.shared_state.active_output_notes.lock() { notes.clear(); }
                    }
                });
            });
        });

        egui::TopBottomPanel::bottom("visualizer_panel").min_height(0.0).show(ctx, |ui| {
            let vis_enabled = self.shared_state.visualizer_enabled.load(Ordering::Relaxed);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Log:").weak());
                ui.label(&self.status_message);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut vis = self.shared_state.visualizer_enabled.load(Ordering::Relaxed);
                    if ui.checkbox(&mut vis, "Show Visualizer").changed() {
                        self.shared_state.visualizer_enabled.store(vis, Ordering::Relaxed);
                    }
                    if vis_enabled {
                        ui.separator();
                        egui::ComboBox::new("vis_mode", "").selected_text("Modes").show_ui(ui, |ui| {
                             let mut sm = self.shared_state.visualizer_show_midi.load(Ordering::Relaxed);
                             if ui.checkbox(&mut sm, "Midi In").changed() { self.shared_state.visualizer_show_midi.store(sm, Ordering::Relaxed); }
                             let mut sr = self.shared_state.visualizer_show_roblox.load(Ordering::Relaxed);
                             if ui.checkbox(&mut sr, "Roblox Out").changed() { self.shared_state.visualizer_show_roblox.store(sr, Ordering::Relaxed); }
                        });
                    }
                });
            });

            if vis_enabled {
                ui.separator();
                let (response, painter) = ui.allocate_painter(egui::vec2(ui.available_width(), 80.0), egui::Sense::hover());
                let rect = response.rect;
                let white_key_width = rect.width() / 52.0; 
                let black_key_width = white_key_width * 0.6;
                let (white_key_height, black_key_height) = (rect.height(), rect.height() * 0.6);
                let input_set = self.shared_state.active_notes.lock().unwrap().clone();
                let output_set = self.shared_state.active_output_notes.lock().unwrap().clone();
                let (show_input, show_output) = (self.shared_state.visualizer_show_midi.load(Ordering::Relaxed), self.shared_state.visualizer_show_roblox.load(Ordering::Relaxed));

                let draw_key = |key_rect: egui::Rect, note: u8, is_black: bool| {
                    let inp = show_input && input_set.contains(&note);
                    let outp = show_output && output_set.contains(&note);
                    let base_color = if is_black { egui::Color32::BLACK } else { egui::Color32::WHITE };
                    let input_color = egui::Color32::GREEN;
                    let output_color = egui::Color32::from_rgb(0, 100, 255); 
                    if inp && outp && show_input && show_output {
                        let half_h = key_rect.height() / 2.0;
                        painter.rect_filled(egui::Rect::from_min_size(key_rect.min, egui::vec2(key_rect.width(), half_h)), if is_black {1.0} else {2.0}, input_color);
                        painter.rect_filled(egui::Rect::from_min_size(egui::pos2(key_rect.min.x, key_rect.min.y + half_h), egui::vec2(key_rect.width(), half_h)), if is_black {1.0} else {2.0}, output_color);
                    } else if inp { painter.rect_filled(key_rect, if is_black {1.0} else {2.0}, input_color); }
                    else if outp { painter.rect_filled(key_rect, if is_black {1.0} else {2.0}, output_color); }
                    else { painter.rect_filled(key_rect, if is_black {1.0} else {2.0}, base_color); }
                    painter.rect(key_rect, 1.0, egui::Color32::TRANSPARENT, egui::Stroke::new(1.0, egui::Color32::GRAY), egui::StrokeKind::Inside);
                };

                let mut x_pos = rect.min.x;
                for note in 21..=108u8 {
                     if !match note % 12 { 1 | 3 | 6 | 8 | 10 => true, _ => false } {
                         draw_key(egui::Rect::from_min_size(egui::pos2(x_pos, rect.min.y), egui::vec2(white_key_width, white_key_height)), note, false);
                         x_pos += white_key_width;
                     }
                }
                let mut white_key_idx = 0;
                for note in 21..=108u8 {
                    if match note % 12 { 1 | 3 | 6 | 8 | 10 => true, _ => false } {
                         let center_x = rect.min.x + (white_key_idx as f32 * white_key_width);
                         draw_key(egui::Rect::from_min_size(egui::pos2(center_x - (black_key_width/2.0), rect.min.y), egui::vec2(black_key_width, black_key_height)), note, true);
                    } else { white_key_idx += 1; }
                }
            }
        });

        // The central panel fills all remaining space
        egui::CentralPanel::default().show(ctx, |ui| {
            // Panic Overlay | scarryyyy :)
            if self.shared_state.panic_active.load(Ordering::Relaxed) {
                egui::Area::new(egui::Id::new("panic_overlay")).anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0)).show(ctx, |ui| {
                    egui::Frame::window(&ctx.style()).fill(egui::Color32::from_rgba_unmultiplied(100, 0, 0, 200)).show(ui, |ui| {
                        ui.vertical_centered(|ui| {
                            ui.heading(egui::RichText::new(" Panic active.").color(egui::Color32::WHITE).size(30.0));
                            if ui.button(egui::RichText::new("Continue").size(20.0)).clicked() { self.shared_state.panic_active.store(false, Ordering::Relaxed); }
                        });
                    });
                });
            }

            // MIDI
            if self.shared_state.use_midi_device.load(Ordering::Relaxed) {
                if let Some(_) = &self.connection {
                    ui.horizontal(|ui| {
                         ui.label(egui::RichText::new("Status: Connected").color(egui::Color32::GREEN));
                         if ui.button("Disconnect").clicked() {
                             self.connection = None;
                             self.status_message = "Disconnected".to_string();
                             if self.midi_input.is_none() { self.midi_input = Some(MidiInput::new("Miditoroblox Input").unwrap()); }
                             self.refresh_ports();
                         }
                    });
                    ui.separator();
                } else {
                     ui.label("Status: Not Connected");
                     let connect_enabled = self.selected_port_name.is_some();
                     if ui.add_enabled(connect_enabled, egui::Button::new("Connect")).clicked() {
                        if let Some(port_name) = &self.selected_port_name {
                            if let Some((_, port)) = self.available_ports.iter().find(|(n, _)| n == port_name) {
                                 if let Some(midi_in) = self.midi_input.take() {
                                     let shared_clone = self.shared_state.clone();
                                     match midi_in.connect(port, "miditoroblox-in", move |_stamp, message, shared_state| {
                                          let _ = shared_state.midi_sender.send(message.to_vec());
                                      }, shared_clone) {
                                         Ok(conn) => { self.connection = Some(conn); self.status_message = format!("Connected to {}", port_name); },
                                         Err(e) => { self.status_message = format!("Error connecting: {}", e); self.midi_input = Some(e.into_inner()); }
                                     }
                                 }
                            }
                        }
                     }
                }
            }

            // Settings
            if self.connection.is_some() || !self.shared_state.use_midi_device.load(Ordering::Relaxed) {
                egui::ScrollArea::vertical().max_height(ui.available_height()).show(ui, |ui| {
                    // MIDI File Player
                    if !self.shared_state.use_midi_device.load(Ordering::Relaxed) {
                        ui.collapsing("MIDI File Player", |ui| {
                            if self.shared_state.app_focused.load(Ordering::Relaxed) {
                                ui.colored_label(egui::Color32::YELLOW, "⚠️ Pause/Play requires Roblox focus. Go back to Roblox to continue playing.");
                            }
                            // 1. Scan dirs list
                            ui.label("Scan Directories:");
                        let mut to_remove = None;
                        for (i, dir) in self.midi_player.scan_dirs.iter_mut().enumerate() {
                            ui.horizontal(|ui| {
                                ui.label(dir.path.to_string_lossy().to_string());
                                ui.checkbox(&mut dir.recursive, "Recursive");
                                if ui.button("🗑").clicked() {
                                    to_remove = Some(i);
                                }
                            });
                        }
                        if let Some(i) = to_remove {
                            self.midi_player.scan_dirs.remove(i);
                        }

                        // 2. Add dir row
                        ui.horizontal(|ui| {
                            ui.text_edit_singleline(&mut self.new_dir_input);
                            ui.checkbox(&mut self.new_dir_recursive, "Recursive");
                            if ui.button("Add Dir").clicked() {
                                if !self.new_dir_input.trim().is_empty() {
                                    self.midi_player.scan_dirs.push(midi_player::ScanDir {
                                        path: std::path::PathBuf::from(self.new_dir_input.trim()),
                                        recursive: self.new_dir_recursive,
                                    });
                                    self.new_dir_input.clear();
                                }
                            }
                        });

                        ui.horizontal(|ui| {
                            if ui.button("Scan Files").clicked() {
                                self.midi_player.scan();
                            }
                            ui.label(format!("Files found: {}", self.midi_player.midi_files.len()));
                        });

                        ui.separator();

                        // 3. Search Bar
                        ui.horizontal(|ui| {
                            ui.label("Search:");
                            ui.text_edit_singleline(&mut self.midi_player.search_query);
                        });

                        // 4. Scrollable file list
                        let filtered = self.midi_player.filtered_files();
                        egui::ScrollArea::vertical().max_height(150.0).show(ui, |ui| {
                            for (idx, path) in filtered {
                                let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("Unknown");
                                let is_selected = self.midi_player.selected_idx == Some(idx);
                                let is_playing = self.midi_player.playing_idx == Some(idx);
                                
                                let mut label_text = filename.to_string();
                                if is_playing {
                                    label_text = format!("▶ {}", label_text);
                                }
                                
                                let resp = ui.selectable_label(is_selected, label_text);
                                if resp.clicked() {
                                    self.midi_player.selected_idx = Some(idx);
                                }
                                if resp.double_clicked() {
                                    self.midi_player.play(idx);
                                }
                            }
                        });

                        // 5. Current file & transport
                        if let Some(idx) = self.midi_player.selected_idx {
                            if let Some(path) = self.midi_player.midi_files.get(idx) {
                                let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("Unknown");
                                ui.label(format!("Selected: {}", filename));
                            }
                        }

                        // Transport buttons
                        ui.horizontal(|ui| {
                            let status = self.midi_player.status();
                            let has_selection = self.midi_player.selected_idx.is_some();

                            if ui.add_enabled(has_selection, egui::Button::new("▶ Play")).clicked() {
                                if let Some(idx) = self.midi_player.selected_idx {
                                    if status == PlaybackStatus::Paused && self.midi_player.playing_idx == Some(idx) {
                                        self.midi_player.resume();
                                    } else {
                                        self.midi_player.play(idx);
                                    }
                                    self.player_paused_by_focus = false;
                                }
                            }

                            if ui.add_enabled(status == PlaybackStatus::Playing || self.player_paused_by_focus, egui::Button::new("⏸ Pause")).clicked() {
                                self.midi_player.pause();
                                self.player_paused_by_focus = false;
                            }

                            if ui.add_enabled(status != PlaybackStatus::Stopped || self.player_paused_by_focus, egui::Button::new("⏹ Stop")).clicked() {
                                self.midi_player.stop();
                                self.player_paused_by_focus = false;
                            }
                        });

                        // 6. Progress / Seek Bar
                        let status = self.midi_player.status();
                        if status != PlaybackStatus::Stopped {
                            let pos = self.midi_player.position();
                            let total = self.midi_player.total();
                            if total > 0 {
                                let mut slider_val = self.player_seek_pos.unwrap_or(pos as f64);
                                let resp = ui.add(egui::Slider::new(&mut slider_val, 0.0..=(total as f64)).show_value(false));
                                if resp.dragged() {
                                    self.player_seek_pos = Some(slider_val);
                                }
                                if resp.drag_released() {
                                    if let Some(seek_to) = self.player_seek_pos.take() {
                                        self.midi_player.seek(seek_to as u64);
                                    }
                                }
                                ui.label(format!("Progress: {} / {}", pos, total));
                            }
                        }
                    });
                    ui.separator();
                    }

                    let mut base = self.shared_state.base_mapping_enabled.load(Ordering::Relaxed);
                    let mut low = self.shared_state.low_mapping_enabled.load(Ordering::Relaxed);
                    let mut high = self.shared_state.high_mapping_enabled.load(Ordering::Relaxed);
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut base, "Middle").changed() { self.shared_state.base_mapping_enabled.store(base, Ordering::Relaxed); }
                        if ui.checkbox(&mut low, "Low").changed() { self.shared_state.low_mapping_enabled.store(low, Ordering::Relaxed); }
                        if ui.checkbox(&mut high, "High").changed() { self.shared_state.high_mapping_enabled.store(high, Ordering::Relaxed); }
                    });

                    let mut auto_t = self.shared_state.auto_transpose_enabled.load(Ordering::Relaxed);
                    if ui.checkbox(&mut auto_t, "Auto-Octave Transpose").changed() { self.shared_state.auto_transpose_enabled.store(auto_t, Ordering::Relaxed); }

                    let mut exp_t = self.shared_state.experimental_transpose_enabled.load(Ordering::Relaxed);
                    if ui.checkbox(&mut exp_t, "Black Keys via Transpose").changed() { self.shared_state.experimental_transpose_enabled.store(exp_t, Ordering::Relaxed); }
                    if exp_t {
                        let mut delay = self.shared_state.transpose_delay_ms.load(Ordering::Relaxed);
                        if ui.add(egui::Slider::new(&mut delay, 0..=1000).text("Ms Delay")).changed() { self.shared_state.transpose_delay_ms.store(delay, Ordering::Relaxed); }
                        let mut lazy = self.shared_state.lazy_transpose_enabled.load(Ordering::Relaxed);
                        if ui.checkbox(&mut lazy, "Optimized").changed() { self.shared_state.lazy_transpose_enabled.store(lazy, Ordering::Relaxed); }
                    }

                    let mut hold_c = self.shared_state.experimental_hold_ctrl_enabled.load(Ordering::Relaxed);
                    if ui.checkbox(&mut hold_c, "Hold CTRL for Ranges").changed() { self.shared_state.experimental_hold_ctrl_enabled.store(hold_c, Ordering::Relaxed); }

                    let mut cv = self.shared_state.control_velocity_enabled.load(Ordering::Relaxed);
                    if ui.checkbox(&mut cv, "Control Velocity").changed() { self.shared_state.control_velocity_enabled.store(cv, Ordering::Relaxed); }
                    if cv {
                        let mut vd = self.shared_state.velocity_delay_ms.load(Ordering::Relaxed);
                        if ui.add(egui::Slider::new(&mut vd, 0..=100).text("Mod Delay")).changed() { self.shared_state.velocity_delay_ms.store(vd, Ordering::Relaxed); }
                    }

                    let mut sus = self.shared_state.sustain_enabled.load(Ordering::Relaxed);
                    if ui.checkbox(&mut sus, "Sustain Pedal Sync").changed() { self.shared_state.sustain_enabled.store(sus, Ordering::Relaxed); }

                    let mut solv = self.shared_state.solver_enabled.load(Ordering::Relaxed);
                    if ui.checkbox(&mut solv, "Smart Solver").changed() { self.shared_state.solver_enabled.store(solv, Ordering::Relaxed); }
                    if solv {
                        ui.indent("solv_s", |ui| {
                            let mut eff = self.shared_state.solver_mode_efficiency.load(Ordering::Relaxed);
                            ui.horizontal(|ui| {
                                if ui.radio_value(&mut eff, true, "Efficiency").clicked() { self.shared_state.solver_mode_efficiency.store(true, Ordering::Relaxed); }
                                if ui.radio_value(&mut eff, false, "Accuracy").clicked() { self.shared_state.solver_mode_efficiency.store(false, Ordering::Relaxed); }
                            });
                            let mut mj = self.shared_state.solver_max_jump.load(Ordering::Relaxed);
                            if ui.add(egui::Slider::new(&mut mj, 1..=24).text("Max Jump")).changed() { self.shared_state.solver_max_jump.store(mj, Ordering::Relaxed); }
                            let mut rng = self.shared_state.transpose_range.load(Ordering::Relaxed);
                            if ui.add(egui::Slider::new(&mut rng, 12..=24).text("Range")).changed() { self.shared_state.transpose_range.store(rng, Ordering::Relaxed); }
                        });
                    }
                    
                    ui.separator();
                    let mut q_en = self.shared_state.quantize_enabled.load(Ordering::Relaxed);
                    if ui.checkbox(&mut q_en, "Quantization").changed() { self.shared_state.quantize_enabled.store(q_en, Ordering::Relaxed); }
                    if q_en {
                        let mut q_ms = self.shared_state.quantize_ms.load(Ordering::Relaxed);
                        if ui.add(egui::Slider::new(&mut q_ms, 10..=500).text("Ms")).changed() { self.shared_state.quantize_ms.store(q_ms, Ordering::Relaxed); }
                    }

                    ui.separator();
                    ui.label(egui::RichText::new("Experimental").strong());
                    let mut drum = self.shared_state.drum_support_enabled.load(Ordering::Relaxed);
                    if ui.checkbox(&mut drum, "Drum Support").changed() {
                        self.shared_state.drum_support_enabled.store(drum, Ordering::Relaxed);
                    }

                    let mut range_inst = self.shared_state.range_instrument_enabled.load(Ordering::Relaxed);
                    if ui.checkbox(&mut range_inst, "Range Instrument Support (Experimental)").changed() {
                        self.shared_state.range_instrument_enabled.store(range_inst, Ordering::Relaxed);
                    }
                    if range_inst {
                        ui.indent("range_inst_slots", |ui| {
                            let mut slots = self.shared_state.instrument_slots.lock().unwrap();
                            for i in 0..4 {
                                ui.group(|ui| {
                                    ui.horizontal(|ui| {
                                        ui.checkbox(&mut slots[i].enabled, format!("Slot {}", i + 1));
                                        if slots[i].enabled {
                                            egui::ComboBox::new(format!("inst_type_{}", i), "Type")
                                                .selected_text(slots[i].inst_type.to_string())
                                                .show_ui(ui, |ui| {
                                                    for t in InstrumentType::all() {
                                                        ui.selectable_value(&mut slots[i].inst_type, *t, t.to_string());
                                                    }
                                                });
                                        }
                                    });
                                    if slots[i].enabled {
                                        ui.horizontal(|ui| {
                                            ui.label("Range:");
                                            ui.add(egui::Slider::new(&mut slots[i].min_note, 1..=88).text("Min"));
                                            ui.add(egui::Slider::new(&mut slots[i].max_note, 1..=88).text("Max"));
                                        });
                                        if slots[i].min_note > slots[i].max_note {
                                            slots[i].min_note = slots[i].max_note;
                                        }
                                        ui.horizontal(|ui| {
                                            ui.label("Transpose:");
                                            ui.add(egui::Slider::new(&mut slots[i].transposition, -24..=24));
                                        });
                                    }
                                });
                            }
                        });
                    }
                    if drum {
                        ui.label("Hover a button and press Left Alt to save its screen position.");
                        let mut lookahead = self.shared_state.lookahead_ms.lock().unwrap_or_else(|e| e.into_inner());
                        if ui.add(egui::Slider::new(&mut *lookahead, 0.0..=200.0).text("Lookahead Buffer (ms)")).changed() {}
                        let mut after_switch_ms = self.shared_state.solo_after_switch_ms.lock().unwrap_or_else(|e| e.into_inner());
                        if ui.add(egui::Slider::new(&mut *after_switch_ms, 0.0..=25.0).text("After Switch (ms)")).changed() {}
                        let mut note_headstart_ms = self.shared_state.note_headstart_ms.lock().unwrap_or_else(|e| e.into_inner());
                        if ui.add(egui::Slider::new(&mut *note_headstart_ms, 0.0..=25.0).text("Note Headstart (ms)")).changed() {}
                        let mut move_delay = self.shared_state.mouse_move_delay_ms.lock().unwrap();
                        if ui.add(egui::Slider::new(&mut *move_delay, 0.0..=25.0).text("Move Delay (ms)")).changed() {}
                        let mut click_hold = self.shared_state.mouse_click_hold_ms.lock().unwrap();
                        if ui.add(egui::Slider::new(&mut *click_hold, 0.0..=25.0).text("Click Hold (ms)")).changed() {}
                        let mut after_release = self.shared_state.mouse_after_release_ms.lock().unwrap();
                        if ui.add(egui::Slider::new(&mut *after_release, 0.0..=25.0).text("After Release (ms)")).changed() {}
                        let mouse_pos = self.shared_state.mouse_position.lock().ok().and_then(|p| *p);
                        let piano_pos = self.shared_state.piano_button_position.lock().ok().and_then(|p| *p);
                        let drum_pos = self.shared_state.drum_button_position.lock().ok().and_then(|p| *p);
                        ui.horizontal(|ui| {
                            if ui.button("Capture Piano").clicked() {
                                self.shared_state.capture_next_left_alt.store(1, Ordering::Relaxed);
                            }
                            if ui.button("Capture Drum").clicked() {
                                self.shared_state.capture_next_left_alt.store(2, Ordering::Relaxed);
                            }
                            if ui.button("Clear").clicked() {
                                if let Ok(mut p) = self.shared_state.piano_button_position.lock() { *p = None; }
                                if let Ok(mut p) = self.shared_state.drum_button_position.lock() { *p = None; }
                                self.shared_state.capture_next_left_alt.store(0, Ordering::Relaxed);
                            }
                        });
                        instrument_box(ui, "Piano Instrument", piano_pos, mouse_pos, true);
                        instrument_box(ui, "Drum Instrument", drum_pos, mouse_pos, false);
                        let stage = self.shared_state.capture_next_left_alt.load(Ordering::Relaxed);
                        if stage == 1 { ui.label("Next Left Alt saves the Piano button."); }
                        if stage == 2 { ui.label("Next Left Alt saves the Drum button."); }
                    }
                });
            }
        });

        if self.show_midi_window {
             ctx.show_viewport_immediate(
                 egui::ViewportId::from_hash_of("midi_selection_window"),
                 egui::ViewportBuilder::default().with_title("Select MIDI").with_inner_size([300.0, 400.0]),
                 |ctx, _class| { egui::CentralPanel::default().show(ctx, |ui| { ui.label("MIDI Selection Window (Placeholder)"); }); }
             );
        }
    }
}

#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Force X11 backend to ensure Always On Top works (stupid wayland)
    unsafe { std::env::remove_var("WAYLAND_DISPLAY") };

    println!("Initializing virtual keyboard (requires permissions to write to /dev/uinput)...");
    
    let mut keys = AttributeSet::<KeyCode>::new();
    keys.insert(KeyCode::KEY_E);
    keys.insert(KeyCode::KEY_LEFTSHIFT);
    keys.insert(KeyCode::KEY_LEFTCTRL);
    keys.insert(KeyCode::KEY_LEFTALT);
    keys.insert(KeyCode::KEY_RIGHTALT);
    keys.insert(KeyCode::KEY_UP);
    keys.insert(KeyCode::KEY_DOWN);
    keys.insert(KeyCode::KEY_APOSTROPHE);
    
    // Register velocity keys
    for code in 2..=11 { keys.insert(KeyCode::new(code)); } // 1-0
    for code in 16..=25 { keys.insert(KeyCode::new(code)); } // Q-P
    for code in 30..=38 { keys.insert(KeyCode::new(code)); } // A-L
    keys.insert(KeyCode::KEY_Z);
    keys.insert(KeyCode::KEY_X);
    keys.insert(KeyCode::KEY_C);
    
    // Register all mapped keys
    for mapping in solver::get_available_mappings() {
        keys.insert(mapping.key_code);
    }

    // Create the virtual device using the builder
    let device = VirtualDevice::builder()?
        .name("Miditoroblox Rust Presser")
        .with_keys(&keys)?
        .build()?;

    let mut options = eframe::NativeOptions::default();
    options.viewport = egui::ViewportBuilder::default()
        .with_transparent(true)
        .with_inner_size([1000.0, 600.0]);
    eframe::run_native(
        "Miditoroblox",
        options,
        Box::new(|cc| Ok(Box::new(MidiApp::new(cc, device)))),
    ).map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

    Ok(())
}

#[cfg(target_os = "windows")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Initializing virtual keyboard on Windows...");
    
    let device = VirtualDevice;

    let mut options = eframe::NativeOptions::default();
    options.viewport = egui::ViewportBuilder::default()
        .with_transparent(true)
        .with_inner_size([1000.0, 600.0]);
    eframe::run_native(
        "Miditoroblox",
        options,
        Box::new(|cc| Ok(Box::new(MidiApp::new(cc, device)))),
    ).map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

    Ok(())
}

#[cfg(target_os = "linux")]
fn is_velocity_key(code: u16) -> bool {
    match code {
        c if c >= 2 && c <= 11 => true,  // 1-0
        c if c >= 16 && c <= 25 => true, // Q-P
        c if c >= 30 && c <= 38 => true, // A-L
        c if c == 44 || c == 45 || c == 46 => true, // Z, X, C
        _ => false,
    }
}

#[cfg(target_os = "windows")]
fn is_velocity_key(code: u16) -> bool {
    VELOCITY_KEYS.contains(&code)
}

fn select_solo_toggle(shared: &SharedState, piano: bool) {
    let pos = if piano {
        shared.piano_button_position.lock().ok().and_then(|p| *p)
    } else {
        shared.drum_button_position.lock().ok().and_then(|p| *p)
    };
    if let Some((x, y)) = pos {
        move_mouse_and_click(shared, x, y);
    }
}

fn set_active_solo(shared: &SharedState, target: Option<bool>) {
    let current = *shared.active_solo.lock().unwrap_or_else(|e| e.into_inner());
    if current == target {
        return;
    }

    // Just click the target instrument select button directly — no deselect step needed.
    if let Some(next) = target {
        select_solo_toggle(shared, next);
    }

    let delay = *shared.solo_after_switch_ms.lock().unwrap_or_else(|e| e.into_inner());
    if delay > 0.0 {
        thread::sleep(time::Duration::from_secs_f32(delay / 1000.0));
    }

    if let Ok(mut guard) = shared.active_solo.lock() {
        *guard = target;
    }
}

fn process_message(shared_state: &SharedState, message: Vec<u8>) {
    if message.len() < 2 { return; }
    let status = message[0] & 0xF0;
    if status == 0xC0 {
        let ch = (message[0] & 0x0F) as usize;
        if ch < 16 {
            if let Ok(mut cp) = shared_state.channel_programs.lock() {
                cp[ch] = message[1];
            }
        }
        return;
    }
    if message.len() < 3 { return; }
    let channel = message[0] & 0x0F;
    if channel == 9 
        && !shared_state.drum_support_enabled.load(Ordering::Relaxed)
        && !shared_state.range_instrument_enabled.load(Ordering::Relaxed) 
    {
        return;
    }
    let note_original = message[1];
    let velocity = message[2];

    let mut targets = Vec::new();
    let is_note_event = status == 0x90 || status == 0x80;
    if is_note_event && shared_state.range_instrument_enabled.load(Ordering::Relaxed) {
        let program = if let Ok(cp) = shared_state.channel_programs.lock() {
            cp[channel as usize]
        } else {
            0
        };
        if let Ok(slots) = shared_state.instrument_slots.lock() {
            for slot in slots.iter() {
                if slot.enabled && match_instrument(channel, program, slot.inst_type) {
                    let note_1_88 = note_original as i32 - 20;
                    let physical = note_1_88 - slot.transposition as i32;
                    if physical >= slot.min_note as i32 && physical <= slot.max_note as i32 {
                        let target_midi = physical + 20;
                        targets.push(target_midi as u8);
                    }
                }
            }
        }
    } else {
        targets.push(note_original);
    }

    for note_original in targets {

    // Visualizer Update
    if status == 0x90 && velocity > 0 {
        if let Ok(mut notes) = shared_state.active_notes.lock() { notes.insert(note_original); }
        if let Ok(ctx_opt) = shared_state.ui_context.lock() { if let Some(ctx) = ctx_opt.as_ref() { ctx.request_repaint(); } }
    } else if status == 0x80 || (status == 0x90 && velocity == 0) {
        if let Ok(mut notes) = shared_state.active_notes.lock() { notes.remove(&note_original); }
        if let Ok(ctx_opt) = shared_state.ui_context.lock() { if let Some(ctx) = ctx_opt.as_ref() { ctx.request_repaint(); } }
    }

    // Pedal thingy
    if status == 0xB0 && shared_state.sustain_enabled.load(Ordering::Relaxed) && message[1] == 64 {
        let pedal_down = message[2] >= 64;
        if pedal_down != shared_state.sustain_active.load(Ordering::Relaxed) {
            if let Ok(mut state) = shared_state.device_state.lock() {
                let _ = state.device.emit(&[
                    InputEvent::new(EventType::KEY.0, KeyCode::KEY_APOSTROPHE.code(), 1),
                    InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                ]);
                let _ = state.device.emit(&[
                    InputEvent::new(EventType::KEY.0, KeyCode::KEY_APOSTROPHE.code(), 0),
                    InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                ]);
                shared_state.sustain_active.store(pedal_down, Ordering::Relaxed);
            }
        }
        return;
    }

    let is_note_valid = |n: u8| -> bool {
        if n < 36 { shared_state.low_mapping_enabled.load(Ordering::Relaxed) }
        else if n > 96 { shared_state.high_mapping_enabled.load(Ordering::Relaxed) }
        else { shared_state.base_mapping_enabled.load(Ordering::Relaxed) }
    };

    let handle_velocity = |state: &mut DeviceState, v: u8, shared: &SharedState| {
        if v > 0 && shared.control_velocity_enabled.load(Ordering::Relaxed) {
            let target_key = VELOCITY_KEYS[((v as u32 * 31) / 127) as usize];
            if target_key != shared.last_sent_velocity_key.load(Ordering::Relaxed) {
                let delay = shared.velocity_delay_ms.load(Ordering::Relaxed);
                let _ = state.device.emit(&[
                    InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTALT.code(), 1),
                    InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                ]);
                if delay > 0 { thread::sleep(time::Duration::from_millis(delay)); }
                let _ = state.device.emit(&[
                    InputEvent::new(EventType::KEY.0, target_key, 1),
                    InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                ]);
                if delay > 0 { thread::sleep(time::Duration::from_millis(delay)); }
                let _ = state.device.emit(&[
                    InputEvent::new(EventType::KEY.0, target_key, 0),
                    InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                ]);
                if delay > 0 { thread::sleep(time::Duration::from_millis(delay)); }
                let _ = state.device.emit(&[
                    InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTALT.code(), 0),
                    InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                ]);
                shared.last_sent_velocity_key.store(target_key, Ordering::Relaxed);
            }
        }
    };

    let (mut final_note, mut valid) = (note_original, is_note_valid(note_original));
    let use_solver = shared_state.solver_enabled.load(Ordering::Relaxed);

    if !use_solver {
        if !valid && shared_state.auto_transpose_enabled.load(Ordering::Relaxed) {
            let mut tn = final_note;
            while tn <= 108 && !is_note_valid(tn) { if let Some(n) = tn.checked_add(12) { tn = n; } else { break; } }
            if is_note_valid(tn) { final_note = tn; valid = true; } 
            else {
                tn = final_note;
                while tn >= 21 && !is_note_valid(tn) { if let Some(n) = tn.checked_sub(12) { tn = n; } else { break; } }
                if is_note_valid(tn) { final_note = tn; valid = true; }
            }
        }
        if !valid { return; }
    }

    if status == 0x90 && velocity > 0 && shared_state.quantize_enabled.load(Ordering::Relaxed) {
        let grid = shared_state.quantize_ms.load(Ordering::Relaxed);
        if grid > 0 { if let Ok(d) = SystemTime::now().duration_since(UNIX_EPOCH) { let rem = (d.as_millis() as u64) % grid; if rem > 0 { thread::sleep(time::Duration::from_millis(grid - rem)); } } }
    }

    if use_solver {
        let mut state = shared_state.device_state.lock().unwrap();
        if status == 0x90 && velocity > 0 {
            let note_headstart_ms = *shared_state.note_headstart_ms.lock().unwrap_or_else(|e| e.into_inner());
            if note_headstart_ms > 0.0 {
                thread::sleep(Duration::from_secs_f32(note_headstart_ms / 1000.0));
            }
            handle_velocity(&mut state, velocity, shared_state);
            let (mode, max_j, range) = (if shared_state.solver_mode_efficiency.load(Ordering::Relaxed) { SolverMode::Efficiency } else { SolverMode::Accuracy }, shared_state.solver_max_jump.load(Ordering::Relaxed) as i32, shared_state.transpose_range.load(Ordering::Relaxed) as i32);
            if let Some((delta, m)) = state.solver.solve(note_original, mode, max_j, range) {
                if let Ok(mut out) = shared_state.active_output_notes.lock() { out.insert(note_original); }
                let curr = state.solver.current_transpose;
                if delta != curr {
                    let diff = delta - curr;
                    let key = if diff > 0 { KeyCode::KEY_UP } else { KeyCode::KEY_DOWN };
                    for _ in 0..diff.abs() {
                        let _ = state.device.emit(&[
                            InputEvent::new(EventType::KEY.0, key.code(), 1),
                            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                        ]);
                        let _ = state.device.emit(&[
                            InputEvent::new(EventType::KEY.0, key.code(), 0),
                            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                        ]);
                        thread::sleep(time::Duration::from_millis(5));
                    }
                    state.current_transpose_offset = delta;
                }
                if state.solver.active_keys.contains_key(&m.key_code) {
                    let _ = state.device.emit(&[
                        InputEvent::new(EventType::KEY.0, m.key_code.code(), 0),
                        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                    ]);
                    thread::sleep(time::Duration::from_millis(5));
                }
                if m.shift && !state.solver.shift_active {
                    let _ = state.device.emit(&[
                        InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 1),
                        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                    ]);
                } else if !m.shift && state.solver.shift_active {
                    let _ = state.device.emit(&[
                        InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 0),
                        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                    ]);
                }
                if m.ctrl && !state.solver.ctrl_active {
                    let _ = state.device.emit(&[
                        InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 1),
                        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                    ]);
                } else if !m.ctrl && state.solver.ctrl_active {
                    let _ = state.device.emit(&[
                        InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 0),
                        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                    ]);
                }
                let _ = state.device.emit(&[
                    InputEvent::new(EventType::KEY.0, m.key_code.code(), 1),
                    InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                ]);
                state.solver.register_note_on(m.key_code, note_original, delta, m.shift, m.ctrl);
            }
        } else {
            if let Some(key) = state.solver.register_note_off(note_original) {
                if let Ok(mut out) = shared_state.active_output_notes.lock() { out.remove(&note_original); }
                let _ = state.device.emit(&[
                    InputEvent::new(EventType::KEY.0, key.code(), 0),
                    InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                ]);
                if !state.solver.shift_active {
                    let _ = state.device.emit(&[
                        InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 0),
                        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                    ]);
                }
                if !state.solver.ctrl_active {
                    let _ = state.device.emit(&[
                        InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 0),
                        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                    ]);
                }
            }
        }
        return;
    }

    let experimental = shared_state.experimental_transpose_enabled.load(Ordering::Relaxed);
    let hold_ctrl = shared_state.experimental_hold_ctrl_enabled.load(Ordering::Relaxed);
    let ms = solver::get_available_mappings();
    if let Some(m) = ms.iter().find(|map| map.midi_note == final_note) {
        let mut state = shared_state.device_state.lock().unwrap();
        if status == 0x90 && velocity > 0 {
            handle_velocity(&mut state, velocity, shared_state);
            if let Ok(mut out) = shared_state.active_output_notes.lock() { out.insert(note_original); }
            let mut handled_t = false;
            if experimental {
                if shared_state.lazy_transpose_enabled.load(Ordering::Relaxed) {
                    let (target, current) = (if m.shift && !m.ctrl { 1 } else { 0 }, state.current_transpose_offset);
                    if target != current {
                        let d = shared_state.transpose_delay_ms.load(Ordering::Relaxed);
                        let k = if target > current { KeyCode::KEY_UP } else { KeyCode::KEY_DOWN };
                        let _ = state.device.emit(&[
                            InputEvent::new(EventType::KEY.0, k.code(), 1),
                            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                        ]);
                        let _ = state.device.emit(&[
                            InputEvent::new(EventType::KEY.0, k.code(), 0),
                            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                        ]);
                        if d > 0 { drop(state); thread::sleep(time::Duration::from_millis(d)); state = shared_state.device_state.lock().unwrap(); }
                        state.current_transpose_offset = target;
                    }
                    handled_t = true;
                } else { state.current_transpose_offset = 0; }
            }
            if m.ctrl {
                if hold_ctrl {
                    let _ = state.device.emit(&[
                        InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 1),
                        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                    ]);
                    let _ = state.device.emit(&[
                        InputEvent::new(EventType::KEY.0, m.key_code.code(), 1),
                        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                    ]);
                    let _ = state.device.emit(&[
                        InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 0),
                        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                    ]);
                } else {
                    let _ = state.device.emit(&[
                        InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 1),
                        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                    ]);
                    let _ = state.device.emit(&[
                        InputEvent::new(EventType::KEY.0, m.key_code.code(), 1),
                        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                    ]);
                    let _ = state.device.emit(&[
                        InputEvent::new(EventType::KEY.0, m.key_code.code(), 0),
                        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                    ]);
                    let _ = state.device.emit(&[
                        InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 0),
                        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                    ]);
                }
            } else if m.shift {
                if experimental {
                    if handled_t {
                        let _ = state.device.emit(&[
                            InputEvent::new(EventType::KEY.0, m.key_code.code(), 1),
                            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                        ]);
                    } else {
                        let d = shared_state.transpose_delay_ms.load(Ordering::Relaxed);
                        let _ = state.device.emit(&[
                            InputEvent::new(EventType::KEY.0, KeyCode::KEY_UP.code(), 1),
                            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                        ]);
                        let _ = state.device.emit(&[
                            InputEvent::new(EventType::KEY.0, KeyCode::KEY_UP.code(), 0),
                            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                        ]);
                        if d > 0 { drop(state); thread::sleep(time::Duration::from_millis(d)); state = shared_state.device_state.lock().unwrap(); }
                        let _ = state.device.emit(&[
                            InputEvent::new(EventType::KEY.0, m.key_code.code(), 1),
                            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                        ]);
                        if d > 0 { drop(state); thread::sleep(time::Duration::from_millis(d)); state = shared_state.device_state.lock().unwrap(); }
                        let _ = state.device.emit(&[
                            InputEvent::new(EventType::KEY.0, KeyCode::KEY_DOWN.code(), 1),
                            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                        ]);
                        let _ = state.device.emit(&[
                            InputEvent::new(EventType::KEY.0, KeyCode::KEY_DOWN.code(), 0),
                            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                        ]);
                    }
                } else {
                    let _ = state.device.emit(&[
                        InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 1),
                        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                    ]);
                    let _ = state.device.emit(&[
                        InputEvent::new(EventType::KEY.0, m.key_code.code(), 1),
                        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                    ]);
                    let _ = state.device.emit(&[
                        InputEvent::new(EventType::KEY.0, m.key_code.code(), 0),
                        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                    ]);
                    let _ = state.device.emit(&[
                        InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 0),
                        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                    ]);
                }
            } else {
                let _ = state.device.emit(&[
                    InputEvent::new(EventType::KEY.0, m.key_code.code(), 1),
                    InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                ]);
            }
        } else {
            if let Ok(mut out) = shared_state.active_output_notes.lock() { out.remove(&note_original); }
            if m.ctrl && hold_ctrl {
                let _ = state.device.emit(&[
                    InputEvent::new(EventType::KEY.0, m.key_code.code(), 0),
                    InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                ]);
            } else if m.shift && experimental {
                let _ = state.device.emit(&[
                    InputEvent::new(EventType::KEY.0, m.key_code.code(), 0),
                    InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                ]);
            } else if !m.shift && !m.ctrl {
                let _ = state.device.emit(&[
                    InputEvent::new(EventType::KEY.0, m.key_code.code(), 0),
                    InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                ]);
            }
        }
    }
}
}

fn current_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn instrument_box(ui: &mut egui::Ui, label: &str, saved: Option<(i32, i32)>, mouse: Option<(i32, i32)>, piano: bool) {
    let rect = ui.available_rect_before_wrap();
    let size = egui::vec2(ui.available_width(), 52.0);
    let (response, painter) = ui.allocate_painter(size, egui::Sense::hover());
    let fill = if piano { egui::Color32::from_rgb(30, 60, 90) } else { egui::Color32::from_rgb(90, 50, 20) };
    painter.rect_filled(response.rect, 6.0, fill);
    painter.rect_stroke(response.rect, 6.0, egui::Stroke::new(1.0, egui::Color32::WHITE), egui::StrokeKind::Inside);
    let text = match (saved, mouse) {
        (Some((sx, sy)), Some((mx, my))) => format!("{label}: saved ({sx}, {sy}) | mouse ({mx}, {my})"),
        (Some((sx, sy)), None) => format!("{label}: saved ({sx}, {sy})"),
        (None, Some((mx, my))) => format!("{label}: unset | mouse ({mx}, {my})"),
        (None, None) => format!("{label}: unset"),
    };
    painter.text(response.rect.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(15.0), egui::Color32::WHITE);
    let _ = rect;
}

#[cfg(target_os = "linux")]
fn move_mouse_and_click(shared: &SharedState, x: i32, y: i32) {
    let Some((screen_w, screen_h)) = get_screen_size() else { return; };
    let max_axis = 65_535i32;
    let abs_x = ((x.clamp(0, screen_w.saturating_sub(1)) * max_axis) / screen_w.max(1)).clamp(0, max_axis);
    let abs_y = ((y.clamp(0, screen_h.saturating_sub(1)) * max_axis) / screen_h.max(1)).clamp(0, max_axis);

    if let Ok(mut device_guard) = shared.mouse_device.lock() {
        if let Some(device) = device_guard.as_mut() {
            let _ = device.emit(&[
                InputEvent::new_now(EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_X.0, abs_x),
                InputEvent::new_now(EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_Y.0, abs_y),
                InputEvent::new_now(EventType::SYNCHRONIZATION.0, 0, 0), // SYN_REPORT
            ]);
            let move_delay = *shared.mouse_move_delay_ms.lock().unwrap_or_else(|e| e.into_inner());
            if move_delay > 0.0 {
                thread::sleep(time::Duration::from_secs_f32(move_delay / 1000.0));
            }
            let _ = device.emit(&[
                InputEvent::new_now(EventType::KEY.0, KeyCode::BTN_LEFT.code(), 1),
                InputEvent::new_now(EventType::SYNCHRONIZATION.0, 0, 0), // SYN_REPORT
            ]);
            let hold_delay = *shared.mouse_click_hold_ms.lock().unwrap_or_else(|e| e.into_inner());
            if hold_delay > 0.0 {
                thread::sleep(time::Duration::from_secs_f32(hold_delay / 1000.0));
            }
            let _ = device.emit(&[
                InputEvent::new_now(EventType::KEY.0, KeyCode::BTN_LEFT.code(), 0),
                InputEvent::new_now(EventType::SYNCHRONIZATION.0, 0, 0), // SYN_REPORT
            ]);
            let after_release_delay = *shared.mouse_after_release_ms.lock().unwrap_or_else(|e| e.into_inner());
            if after_release_delay > 0.0 {
                thread::sleep(time::Duration::from_secs_f32(after_release_delay / 1000.0));
            }
        }
    }

    if let Ok(mut pos) = shared.mouse_position.lock() {
        *pos = Some((x, y));
    }
}

#[cfg(target_os = "windows")]
fn move_mouse_and_click(shared: &SharedState, x: i32, y: i32) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_MOUSE, MOUSEINPUT, MOUSEEVENTF_ABSOLUTE,
        MOUSEEVENTF_MOVE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    let max_axis = 65_535i32;
    let screen_w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let screen_h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    if screen_w <= 0 || screen_h <= 0 { return; }

    let abs_x = (x.clamp(0, screen_w - 1) * max_axis) / screen_w;
    let abs_y = (y.clamp(0, screen_h - 1) * max_axis) / screen_h;

    // Move mouse
    {
        let mut input: INPUT = unsafe { std::mem::zeroed() };
        input.r#type = INPUT_MOUSE;
        let mut mi: MOUSEINPUT = unsafe { std::mem::zeroed() };
        mi.dx = abs_x;
        mi.dy = abs_y;
        mi.dwFlags = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE;
        input.Anonymous.mi = mi;
        unsafe {
            SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
        }
    }

    let move_delay = *shared.mouse_move_delay_ms.lock().unwrap_or_else(|e| e.into_inner());
    if move_delay > 0.0 {
        thread::sleep(time::Duration::from_secs_f32(move_delay / 1000.0));
    }

    // Press down
    {
        let mut input: INPUT = unsafe { std::mem::zeroed() };
        input.r#type = INPUT_MOUSE;
        let mut mi: MOUSEINPUT = unsafe { std::mem::zeroed() };
        mi.dwFlags = MOUSEEVENTF_LEFTDOWN;
        input.Anonymous.mi = mi;
        unsafe {
            SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
        }
    }

    let hold_delay = *shared.mouse_click_hold_ms.lock().unwrap_or_else(|e| e.into_inner());
    if hold_delay > 0.0 {
        thread::sleep(time::Duration::from_secs_f32(hold_delay / 1000.0));
    }

    // Release up
    {
        let mut input: INPUT = unsafe { std::mem::zeroed() };
        input.r#type = INPUT_MOUSE;
        let mut mi: MOUSEINPUT = unsafe { std::mem::zeroed() };
        mi.dwFlags = MOUSEEVENTF_LEFTUP;
        input.Anonymous.mi = mi;
        unsafe {
            SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
        }
    }

    let after_release_delay = *shared.mouse_after_release_ms.lock().unwrap_or_else(|e| e.into_inner());
    if after_release_delay > 0.0 {
        thread::sleep(time::Duration::from_secs_f32(after_release_delay / 1000.0));
    }

    if let Ok(mut pos) = shared.mouse_position.lock() {
        *pos = Some((x, y));
    }
}

#[cfg(target_os = "linux")]
fn build_mouse_device() -> std::io::Result<VirtualDevice> {
    let mut keys = AttributeSet::<KeyCode>::new();
    keys.insert(KeyCode::BTN_LEFT);

    let mut props = AttributeSet::<PropType>::new();
    props.insert(PropType::POINTER);

    let abs = AbsInfo::new(0, 0, 65_535, 0, 0, 0);
    let abs_x = UinputAbsSetup::new(AbsoluteAxisCode::ABS_X, abs);
    let abs_y = UinputAbsSetup::new(AbsoluteAxisCode::ABS_Y, abs);

    VirtualDevice::builder()?
        .name("Miditoroblox Mouse")
        .with_keys(&keys)?
        .with_properties(&props)?
        .with_absolute_axis(&abs_x)?
        .with_absolute_axis(&abs_y)?
        .build()
}

#[cfg(target_os = "windows")]
fn build_mouse_device() -> std::io::Result<VirtualDevice> {
    Ok(VirtualDevice)
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn build_mouse_device() -> std::io::Result<VirtualDevice> {
    Err(std::io::Error::new(std::io::ErrorKind::Other, "mouse uinput only supported on linux"))
}

#[cfg(target_os = "linux")]
fn get_screen_size() -> Option<(i32, i32)> {
    let (conn, screen_num) = x11rb::connect(None).ok()?;
    let screen = &conn.setup().roots[screen_num];
    Some((i32::from(screen.width_in_pixels), i32::from(screen.height_in_pixels)))
}

#[cfg(target_os = "windows")]
fn get_screen_size() -> Option<(i32, i32)> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    let w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    if w > 0 && h > 0 {
        Some((w, h))
    } else {
        None
    }
}

#[cfg(target_os = "linux")]
const VELOCITY_KEYS: [u16; 32] = [
    2, 3, 4, 5, 6, 7, 8, 9, 10, 11, // 1-0
    16, 17, 18, 19, 20, 21, 22, 23, 24, 25, // Q-P
    30, 31, 32, 33, 34, 35, 36, 37, 38, // A-L
    44, 45, 46 // Z, X, C
];

#[cfg(target_os = "windows")]
const VELOCITY_KEYS: [u16; 32] = [
    0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x30, // 1-0
    0x51, 0x57, 0x45, 0x52, 0x54, 0x59, 0x55, 0x49, 0x4F, 0x50, // Q-P
    0x41, 0x53, 0x44, 0x46, 0x47, 0x48, 0x4A, 0x4B, 0x4C, // A-L
    0x5A, 0x58, 0x43 // Z, X, C
];

fn release_all_virtual_keys(state: &mut DeviceState, shared_state: &SharedState) {
    let keys = state.solver.reset_keys();
    for k in keys {
        let _ = state.device.emit(&[InputEvent::new(1, k.code(), 0)]);
    }
    let _ = state.device.emit(&[
        InputEvent::new(1, KeyCode::KEY_LEFTSHIFT.code(), 0),
        InputEvent::new(1, KeyCode::KEY_LEFTCTRL.code(), 0),
        InputEvent::new(1, KeyCode::KEY_LEFTALT.code(), 0),
    ]);
    for map in solver::get_available_mappings() {
        let _ = state.device.emit(&[InputEvent::new(1, map.key_code.code(), 0)]);
    }
    for vk in VELOCITY_KEYS {
        let _ = state.device.emit(&[InputEvent::new(1, vk, 0)]);
    }
    let _ = state.device.emit(&[InputEvent::new(1, KeyCode::KEY_APOSTROPHE.code(), 0)]);
    #[cfg(target_os = "linux")]
    let _ = state.device.emit(&[InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0)]);
    shared_state.last_sent_velocity_key.store(0, Ordering::Relaxed);
}

fn match_instrument(channel: u8, program: u8, target: InstrumentType) -> bool {
    if channel == 9 {
        return target == InstrumentType::Drum;
    }
    match target {
        InstrumentType::Piano => program <= 23,
        InstrumentType::Guitar => program >= 24 && program <= 31,
        InstrumentType::Bass => program >= 32 && program <= 39,
        InstrumentType::Strings => program >= 40 && program <= 55,
        InstrumentType::Brass => program >= 56 && program <= 79,
        InstrumentType::Synth => program >= 80 && program <= 103,
        InstrumentType::Drum => program >= 112 && program <= 119,
        InstrumentType::Other => (program > 103 && program < 112) || program > 119,
    }
}
