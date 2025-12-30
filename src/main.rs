use eframe::egui;
use evdev::{uinput::VirtualDevice, AttributeSet, EventType, InputEvent, KeyCode};
use midir::{MidiInput, MidiInputConnection, MidiInputPort};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{self, SystemTime, UNIX_EPOCH};
use std::thread;

mod solver;
use solver::{Solver, SolverMode};

// Mappings moved to solver.rs


struct DeviceState {
    device: VirtualDevice,
    ctrl_count: usize,
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
    // Solver Settings
    solver_enabled: AtomicBool,
    solver_mode_efficiency: AtomicBool, // true = Efficiency, false = Accuracy
    solver_max_jump: AtomicU64,
    transpose_range: AtomicU64, // e.g. 24
    active_notes: Mutex<std::collections::HashSet<u8>>,
    ui_context: Mutex<Option<egui::Context>>,
}
struct MidiApp {
    midi_input: Option<MidiInput>,
    available_ports: Vec<(String, MidiInputPort)>,
    selected_port_name: Option<String>,
    connection: Option<MidiInputConnection<Arc<SharedState>>>,
    shared_state: Arc<SharedState>,
    status_message: String,
    // Window Settings
    window_opacity: f32,
    always_on_top: bool,
}

impl MidiApp {
    fn new(cc: &eframe::CreationContext<'_>, virtual_device: VirtualDevice) -> Self {
        let mut app = Self {
            midi_input: Some(MidiInput::new("Miditoroblox Input").unwrap()),
            available_ports: Vec::new(),
            selected_port_name: None,
            connection: None,
            shared_state: Arc::new(SharedState {
                device_state: Mutex::new(DeviceState {
                    device: virtual_device,
                    ctrl_count: 0,
                    current_transpose_offset: 0,
                    solver: Solver::new(),
                }),
                base_mapping_enabled: AtomicBool::new(false),
                low_mapping_enabled: AtomicBool::new(false),
                high_mapping_enabled: AtomicBool::new(false),
                auto_transpose_enabled: AtomicBool::new(false),
                experimental_transpose_enabled: AtomicBool::new(false),
                experimental_hold_ctrl_enabled: AtomicBool::new(false),
                transpose_delay_ms: AtomicU64::new(0),
                lazy_transpose_enabled: AtomicBool::new(false),
                quantize_enabled: AtomicBool::new(false),
                quantize_ms: AtomicU64::new(100),
                solver_enabled: AtomicBool::new(false),
                solver_mode_efficiency: AtomicBool::new(true),
                solver_max_jump: AtomicU64::new(10), // Default 10 semitones jump limit
                transpose_range: AtomicU64::new(24),
                active_notes: Mutex::new(std::collections::HashSet::new()),
                ui_context: Mutex::new(None),
            }),
            status_message: "Ready".to_string(),
            window_opacity: 1.0,
            always_on_top: false,
        };
        
        // Initialize visuals (opaque default)
        let mut visuals = egui::Visuals::dark();
        visuals.window_fill = egui::Color32::from_black_alpha(255);
        visuals.panel_fill = egui::Color32::from_black_alpha(255);
        cc.egui_ctx.set_visuals(visuals);

        app.refresh_ports();
        app
    }

    fn refresh_ports(&mut self) {
        if self.connection.is_some() {
            return;
        }

        let midi_in = match &self.midi_input {
            Some(m) => m,
            None => {
                // If we don't have one (shouldn't happen unless we failed to create it earlier), try create one
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

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Miditoroblox");

            ui.add_space(10.0);

            // MIDI Selection Area
            ui.horizontal(|ui| {
                ui.label("Midi Device:");
                
                let ports_available = !self.available_ports.is_empty();
                
                egui::ComboBox::from_id_salt("midi_selector")
                    .selected_text(self.selected_port_name.as_deref().unwrap_or(if ports_available { "Select Port" } else { "No Ports Found" }))
                    .show_ui(ui, |ui| {
                        for (name, _) in &self.available_ports {
                            ui.selectable_value(&mut self.selected_port_name, Some(name.clone()), name);
                        }
                    });

                if ui.button("Refresh").clicked() {
                    self.refresh_ports();
                }
            });

            ui.add_space(10.0);

            // Connection controls
            if self.connection.is_some() {
                ui.label(format!("Status: Connected to {}", self.selected_port_name.as_deref().unwrap_or("Unknown")));
                
                // Toggle for MIDI Mappings
                let mut base_enabled = self.shared_state.base_mapping_enabled.load(Ordering::Relaxed);
                if ui.checkbox(&mut base_enabled, "Enable Base (C2-C7)").changed() {
                    self.shared_state.base_mapping_enabled.store(base_enabled, Ordering::Relaxed);
                }
                
                let mut low_enabled = self.shared_state.low_mapping_enabled.load(Ordering::Relaxed);
                if ui.checkbox(&mut low_enabled, "Enable Low Range (A0-B1)").changed() {
                    self.shared_state.low_mapping_enabled.store(low_enabled, Ordering::Relaxed);
                }
                
                let mut high_enabled = self.shared_state.high_mapping_enabled.load(Ordering::Relaxed);
                if ui.checkbox(&mut high_enabled, "Enable High Range (C#7-C8)").changed() {
                    self.shared_state.high_mapping_enabled.store(high_enabled, Ordering::Relaxed);
                }

                let mut auto_transpose = self.shared_state.auto_transpose_enabled.load(Ordering::Relaxed);
                if ui.checkbox(&mut auto_transpose, "Enable Auto-Octave Transposition").changed() {
                    self.shared_state.auto_transpose_enabled.store(auto_transpose, Ordering::Relaxed);
                }
                
                let mut experimental_transpose = self.shared_state.experimental_transpose_enabled.load(Ordering::Relaxed);
                if ui.checkbox(&mut experimental_transpose, "Experimental: Transpose Method for Black Keys").changed() {
                    self.shared_state.experimental_transpose_enabled.store(experimental_transpose, Ordering::Relaxed);
                }



                if experimental_transpose {
                     let mut delay = self.shared_state.transpose_delay_ms.load(Ordering::Relaxed);
                     if ui.add(egui::Slider::new(&mut delay, 0..=200).text("Transpose Delay (ms)")).changed() {
                         self.shared_state.transpose_delay_ms.store(delay, Ordering::Relaxed);
                     }
                     
                     let mut lazy = self.shared_state.lazy_transpose_enabled.load(Ordering::Relaxed);
                     if ui.checkbox(&mut lazy, "Enable Lazy Optimization (Reduce Key Presses)").changed() {
                         self.shared_state.lazy_transpose_enabled.store(lazy, Ordering::Relaxed);
                     }
                     
                     if lazy {
                         ui.label(egui::RichText::new("⚠️ Ensure in-game transposition is 0 before starting!").color(egui::Color32::YELLOW));
                     }
                }

                let mut hold_ctrl = self.shared_state.experimental_hold_ctrl_enabled.load(Ordering::Relaxed);
                if ui.checkbox(&mut hold_ctrl, "Experimental: Hold Notes for Low/High Range (Ctrl)").changed() {
                    self.shared_state.experimental_hold_ctrl_enabled.store(hold_ctrl, Ordering::Relaxed);
                }
                
                ui.separator();
                let mut quantize = self.shared_state.quantize_enabled.load(Ordering::Relaxed);
                if ui.checkbox(&mut quantize, "Enable Note Quantization (Grid Snap)").changed() {
                    self.shared_state.quantize_enabled.store(quantize, Ordering::Relaxed);
                }
                if quantize {
                     let mut q_ms = self.shared_state.quantize_ms.load(Ordering::Relaxed);
                     if ui.add(egui::Slider::new(&mut q_ms, 10..=1000).text("Grid Size (ms)")).changed() {
                         self.shared_state.quantize_ms.store(q_ms, Ordering::Relaxed);
                     }
                }
                
                ui.separator();
                ui.heading("Smart Solver");
                let mut solver_on = self.shared_state.solver_enabled.load(Ordering::Relaxed);
                if ui.checkbox(&mut solver_on, "Enable Smart Solver (Experimental)").changed() {
                    self.shared_state.solver_enabled.store(solver_on, Ordering::Relaxed);
                }
                
                if solver_on {
                    ui.label(egui::RichText::new("⚠️ IMPORTANT: Ensure in-game transposition is 0 before starting!").color(egui::Color32::RED));
                    
                    let mut is_efficiency = self.shared_state.solver_mode_efficiency.load(Ordering::Relaxed);
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut is_efficiency, true, "Efficiency (Least Clicks)");
                        ui.radio_value(&mut is_efficiency, false, "Accuracy (Best Match)");
                    });
                     if is_efficiency != self.shared_state.solver_mode_efficiency.load(Ordering::Relaxed) {
                        self.shared_state.solver_mode_efficiency.store(is_efficiency, Ordering::Relaxed);
                    }
                    
                    if is_efficiency {
                         let mut max_jump = self.shared_state.solver_max_jump.load(Ordering::Relaxed);
                         if ui.add(egui::Slider::new(&mut max_jump, 1..=24).text("Max Jump Distance")).changed() {
                             self.shared_state.solver_max_jump.store(max_jump, Ordering::Relaxed);
                         }
                    }
                    
                    let mut range = self.shared_state.transpose_range.load(Ordering::Relaxed);
                    if ui.add(egui::Slider::new(&mut range, 12..=36).text("Transposition Range (+/-)")).changed() {
                        self.shared_state.transpose_range.store(range, Ordering::Relaxed);
                    }
                    
                    ui.add_space(5.0);
                    ui.horizontal(|ui| {
                        if ui.button("Reset Solver Transpose (0)").clicked() {
                            let mut state = self.shared_state.device_state.lock().unwrap();
                            state.solver.reset_transpose();
                            state.current_transpose_offset = 0; // Sync
                        }
                        if ui.button("Panic: Release All Keys").clicked() {
                            let mut state = self.shared_state.device_state.lock().unwrap();
                            let keys = state.solver.reset_keys();
                            for k in keys {
                                let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, k.code(), 0)]);
                            }
                            // Also ensure modifiers are down
                            let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 0)]);
                            let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 0)]);
                        }
                    });
                }

                if ui.button("Disconnect").clicked() {
                    self.connection = None;
                    // Re-create input for future scanning
                    self.midi_input = Some(MidiInput::new("Miditoroblox Input").unwrap()); 
                    self.refresh_ports();
                    self.status_message = "Disconnected".to_string();
                }
            } else {
                 ui.label("Status: Not Connected");
                 let connect_enabled = self.selected_port_name.is_some();
                 if ui.add_enabled(connect_enabled, egui::Button::new("Connect")).clicked() {
                    if let Some(port_name) = &self.selected_port_name {
                        if let Some((_, port)) = self.available_ports.iter().find(|(n, _)| n == port_name) {
                             if let Some(midi_in) = self.midi_input.take() {
                                 let shared_clone = self.shared_state.clone();
                                 // connect
                                 match midi_in.connect(port, "miditoroblox-in", move |_stamp, message, shared_state| {
                                     // MIDI Callback
                                     // 0x90 is Note On channel 0. 0x80 is Note Off channel 0.
                                     
                                     if message.len() < 3 { return; }
                                     let status = message[0] & 0xF0;
                                     let channel = message[0] & 0x0F;
                                     let note_original = message[1];
                                     let velocity = message[2];

                                     // Update Visualizer State
                                     if status == 0x90 && velocity > 0 {
                                         if let Ok(mut notes) = shared_state.active_notes.lock() {
                                             notes.insert(note_original);
                                         }
                                         // Request UI Repaint
                                         if let Ok(ctx_opt) = shared_state.ui_context.lock() {
                                             if let Some(ctx) = ctx_opt.as_ref() {
                                                 ctx.request_repaint();
                                             }
                                         }
                                     } else if status == 0x80 || (status == 0x90 && velocity == 0) {
                                         if let Ok(mut notes) = shared_state.active_notes.lock() {
                                             notes.remove(&note_original);
                                         }
                                         // Request UI Repaint
                                         if let Ok(ctx_opt) = shared_state.ui_context.lock() {
                                              if let Some(ctx) = ctx_opt.as_ref() {
                                                  ctx.request_repaint();
                                              }
                                         }
                                     }

                                     // Ignore Channel 10 (Drums)
                                     if channel == 9 {
                                         return;
                                     }
                                     
                                     // Helper to check if a note is enabled
                                     let is_note_valid = |n: u8| -> bool {
                                          if n < 36 {
                                              shared_state.low_mapping_enabled.load(Ordering::Relaxed)
                                          } else if n > 96 {
                                              shared_state.high_mapping_enabled.load(Ordering::Relaxed)
                                          } else {
                                              shared_state.base_mapping_enabled.load(Ordering::Relaxed)
                                          }
                                     };
                                     
                                     let mut final_note = note_original;
                                     let mut valid = is_note_valid(final_note);
                                     
                                     // Auto-transpose logic if solver is NOT enabled (legacy behavior)
                                     let use_solver = shared_state.solver_enabled.load(Ordering::Relaxed);

                                     if !use_solver {
                                          if !valid && shared_state.auto_transpose_enabled.load(Ordering::Relaxed) {
                                              // If note is too low, move up
                                              let mut test_note = final_note;
                                              while test_note <= 108 && !is_note_valid(test_note) {
                                                   if let Some(next) = test_note.checked_add(12) {
                                                       test_note = next;
                                                   } else { break; }
                                              }
                                              if is_note_valid(test_note) {
                                                  final_note = test_note;
                                                  valid = true;
                                              } else {
                                                   // Try moving down
                                                   let mut test_note = final_note;
                                                   while test_note >= 21 && !is_note_valid(test_note) {
                                                       if let Some(prev) = test_note.checked_sub(12) {
                                                           test_note = prev;
                                                       } else { break; }
                                                   }
                                                   if is_note_valid(test_note) {
                                                       final_note = test_note;
                                                       valid = true;
                                                   }
                                              }
                                          }
    
                                          if !valid {
                                              return;
                                          }
                                     }
                                     
                                     // Common: Quantization Check
                                     if status == 0x90 && velocity > 0 {
                                          if shared_state.quantize_enabled.load(Ordering::Relaxed) {
                                               let grid = shared_state.quantize_ms.load(Ordering::Relaxed);
                                               if grid > 0 {
                                                   if let Ok(duration) = SystemTime::now().duration_since(UNIX_EPOCH) {
                                                        let now_ms = duration.as_millis() as u64;
                                                        let rem = now_ms % grid;
                                                        if rem > 0 {
                                                            let wait_ms = grid - rem;
                                                            thread::sleep(time::Duration::from_millis(wait_ms));
                                                        }
                                                   }
                                               }
                                           }
                                     }
                                     
                                     if use_solver {
                                         let mut state = shared_state.device_state.lock().unwrap();
                                         
                                         // Solver Logic
                                         if status == 0x90 && velocity > 0 {
                                             // Note On
                                             let mode = if shared_state.solver_mode_efficiency.load(Ordering::Relaxed) {
                                                 SolverMode::Efficiency 
                                             } else { SolverMode::Accuracy };
                                             
                                             let max_jump = shared_state.solver_max_jump.load(Ordering::Relaxed) as i32;
                                             let range = shared_state.transpose_range.load(Ordering::Relaxed) as i32;
                                             
                                             if let Some((delta, mapping)) = state.solver.solve(note_original, mode, max_jump, range) {
                                                 // Execute Solution
                                                 
                                                 // 1. Adjust Transpose
                                                 let target_offset = delta;
                                                 let current = state.solver.current_transpose; // Use solver's internal tracker
                                                 
                                                 if target_offset != current {
                                                     let diff = target_offset - current;
                                                     if diff > 0 {
                                                         for _ in 0..diff {
                                                             let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_UP.code(), 1)]);
                                                             let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_UP.code(), 0)]);
                                                             thread::sleep(time::Duration::from_millis(5)); // small delay for stability
                                                         }
                                                     } else {
                                                         for _ in 0..diff.abs() {
                                                             let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_DOWN.code(), 1)]);
                                                             let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_DOWN.code(), 0)]);
                                                             thread::sleep(time::Duration::from_millis(5));
                                                         }
                                                     }
                                                     // Sync main state
                                                     state.current_transpose_offset = target_offset;
                                                 }
                                                 
                                                 // 2. Press Key
                                                 // Handle Modifiers!
                                                 // We need to sync physical modifiers with mapping.
                                                 // If map.shift is true, we need Shift pressed.
                                                 if mapping.shift && !state.solver.shift_active {
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 1)]);
                                                 } else if !mapping.shift && state.solver.shift_active {
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 0)]);
                                                 }
                                                 
                                                 if mapping.ctrl && !state.solver.ctrl_active {
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 1)]);
                                                 } else if !mapping.ctrl && state.solver.ctrl_active {
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 0)]);
                                                 }
                                                 
                                                 // Press the note key
                                                 let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, mapping.key_code.code(), 1)]);
                                                 
                                                 // Update Solver State
                                                 state.solver.register_note_on(mapping.key_code, note_original, target_offset, mapping.shift, mapping.ctrl);
                                             }
                                         } else if status == 0x80 || (status == 0x90 && velocity == 0) {
                                             // Note Off
                                             if let Some(key_to_release) = state.solver.register_note_off(note_original) {
                                                 let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, key_to_release.code(), 0)]);
                                                 
                                                 // Check if we emptied keys and need to release modifiers?
                                                 // Solver updates internal state, but we need to update physical device
                                                 // Solver state: shift_active, ctrl_active are updated in register_note_off if all keys gone
                                                 
                                                 if !state.solver.shift_active {
                                                     // If physical shift is still held (we don't track physical separately, assume synced with solver state)
                                                     // But wait, if we are 'lazy', we might leave it?
                                                     // No, for Shift/Ctrl we should probably release if not needed to avoid interference with typing or other things?
                                                     // Or just rely on next press? 
                                                     // User said: "play multiple notes that would otherwise conflict".
                                                     // If I release key A, but hold key B (which needs Shift), and key A needed Shift, then Shift stays on.
                                                     // If I release key B (last key), Shift turns off in Solver. So we should send Shift Up.
                                                     
                                                     // We can just forcibly set Modifiers to match Solver State?
                                                     // But we don't know "previous" state easily unless we track it or blindly emit.
                                                     // Blindly emitting 0 is safe if it's already 0.
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 0)]);
                                                 }
                                                 if !state.solver.ctrl_active {
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 0)]);
                                                 }
                                             }
                                         }
                                         
                                         return;
                                     }

                                     let use_experimental_transpose = shared_state.experimental_transpose_enabled.load(Ordering::Relaxed);
                                     let use_hold_ctrl = shared_state.experimental_hold_ctrl_enabled.load(Ordering::Relaxed);

                                     let mappings = solver::get_available_mappings();
                                     if let Some(mapping) = mappings.iter().find(|m| m.midi_note == final_note) {
                                         let mut state = shared_state.device_state.lock().unwrap();
                                         let mapping_code = mapping.key_code;
                                         let mapping_shift = mapping.shift;
                                         let mapping_ctrl = mapping.ctrl;
                                         
                                         // Note On (and velocity > 0)
                                         if status == 0x90 && velocity > 0 {
                                             // ALREADY HANDLED QUANTIZATION ABOVE
                                             
                                             
                                             // --- Exact State Tracking / Lazy Transposition OR Naive ---
                                             let mut handled_transpose = false;
                                             
                                             if use_experimental_transpose {
                                                 let use_lazy = shared_state.lazy_transpose_enabled.load(Ordering::Relaxed);
                                                 
                                                 if use_lazy {
                                                     // Lazy Mode
                                                     let target_offset = if mapping_shift && !mapping_ctrl { 1 } else { 0 };
                                                     let current_offset = state.current_transpose_offset;

                                                     if target_offset != current_offset {
                                                         let delay_ms = shared_state.transpose_delay_ms.load(Ordering::Relaxed);
                                                         
                                                         // We need to move
                                                         if target_offset > current_offset {
                                                             // Need invalid +1 (Assuming we only go 0 <-> 1)
                                                             // Tap UP
                                                             let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_UP.code(), 1)]);
                                                             let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_UP.code(), 0)]);
                                                         } else {
                                                             // Need -1
                                                             // Tap DOWN
                                                             let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_DOWN.code(), 1)]);
                                                             let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_DOWN.code(), 0)]);
                                                         }

                                                         if delay_ms > 0 {
                                                             drop(state);
                                                             thread::sleep(time::Duration::from_millis(delay_ms));
                                                             state = shared_state.device_state.lock().unwrap();
                                                         }
                                                         
                                                         state.current_transpose_offset = target_offset;
                                                     }
                                                     handled_transpose = true;
                                                     
                                                 } else {
                                                     // Naive Mode (Up -> Note -> Down)
                                                     // We handle this inside boolean logic below only for Shift notes
                                                     // But we must reset state tracker just in case user switched modes mid-stream
                                                     state.current_transpose_offset = 0; 
                                                 }
                                             }


                                             if mapping_ctrl {
                                                 if use_hold_ctrl {
                                                     // Experimental Hold Mode: Tap Ctrl, Hold Key
                                                     // 1. Press Ctrl
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 1)]);
                                                     // 2. Press Note (Hold)
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, mapping_code.code(), 1)]);
                                                     // 3. Release Ctrl
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 0)]);
                                                 } else {
                                                     // Ctrl Key: Atomic Ctrl Tap (Original)
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 1)]);
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, mapping_code.code(), 1)]);
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, mapping_code.code(), 0)]);
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 0)]);
                                                 }
                                             } else if mapping_shift {
                                                 if use_experimental_transpose {
                                                     if handled_transpose {
                                                         // Lazy Mode: State is set, just press key
                                                         let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, mapping_code.code(), 1)]);
                                                     } else {
                                                         // Naive Mode: Up -> Note -> Down
                                                         let delay_ms = shared_state.transpose_delay_ms.load(Ordering::Relaxed);
                                                         
                                                         // 1. Up
                                                         let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_UP.code(), 1)]);
                                                         let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_UP.code(), 0)]);
                                                         
                                                         if delay_ms > 0 {
                                                             drop(state);
                                                             thread::sleep(time::Duration::from_millis(delay_ms));
                                                             state = shared_state.device_state.lock().unwrap();
                                                         }
                                                         
                                                         // 2. Note
                                                         let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, mapping_code.code(), 1)]);

                                                         if delay_ms > 0 {
                                                             drop(state);
                                                             thread::sleep(time::Duration::from_millis(delay_ms));
                                                             state = shared_state.device_state.lock().unwrap();
                                                         }
                                                         
                                                         // 3. Down
                                                         let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_DOWN.code(), 1)]);
                                                         let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_DOWN.code(), 0)]);
                                                     }
                                                 } else {
                                                     // Default: Atomic Shift Tap
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 1)]);
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, mapping_code.code(), 1)]);
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, mapping_code.code(), 0)]);
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 0)]);
                                                 }
                                             } else {
                                                  // White Key: Normal Press
                                                  let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, mapping_code.code(), 1)]);
                                             }
                                         }
                                         // Note Off or Note On with velocity 0
                                         else if status == 0x80 || (status == 0x90 && velocity == 0) {
                                              if mapping_ctrl && use_hold_ctrl {
                                                  // Release Mode for Ctrl: Just release the key
                                                  let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, mapping_code.code(), 0)]);
                                              } else if mapping_shift && use_experimental_transpose {
                                                  // Release logic for experimental mode: Stop Holding Key
                                                  let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, mapping_code.code(), 0)]);
                                              } else if !mapping_shift && !mapping_ctrl {
                                                  // White Key: Normal Release
                                                  let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, mapping_code.code(), 0)]);
                                              }
                                              // Standard Black/Ctrl keys (in non-hold mode) are auto-released on NoteOn, so ignore NoteOff
                                         }
                                     }

                                 }, shared_clone) {
                                     Ok(conn) => {
                                         self.connection = Some(conn);
                                         self.status_message = format!("Connected to {}", port_name);
                                     },
                                     Err(e) => {
                                         self.status_message = format!("Error connecting: {}", e);
                                         self.midi_input = Some(e.into_inner()); 
                                     }
                                 }
                             }
                        }
                    }
                }
            }

            
            ui.add_space(10.0);
            
            ui.separator();
            ui.heading("Window Settings");
            
            if ui.checkbox(&mut self.always_on_top, "Always On Top").changed() {
                let level = if self.always_on_top {
                    egui::WindowLevel::AlwaysOnTop
                } else {
                    egui::WindowLevel::Normal
                };
                ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
            }
            
            ui.horizontal(|ui| {
                ui.label("Opacity:");
                if ui.add(egui::Slider::new(&mut self.window_opacity, 0.1..=1.0)).changed() {
                    let mut visuals = egui::Visuals::dark();
                    let alpha = (self.window_opacity * 255.0) as u8;
                    visuals.window_fill = egui::Color32::from_black_alpha(alpha);
                    visuals.panel_fill = egui::Color32::from_black_alpha(alpha);
                    ctx.set_visuals(visuals);
                }
            });

            ui.add_space(10.0);
            ui.label(format!("Log: {}", self.status_message));

            ui.add_space(10.0);
            ui.separator();
            ui.heading("Visualizer");
            
            egui::ScrollArea::horizontal().show(ui, |ui| {
                let (response, painter) = ui.allocate_painter(egui::vec2(800.0, 100.0), egui::Sense::hover());
                let rect = response.rect;
                
                let white_key_width = rect.width() / 52.0; // 52 white keys from A0 to C8
                let black_key_width = white_key_width * 0.6;
                let white_key_height = rect.height();
                let black_key_height = rect.height() * 0.6;
                
                let active_set = if let Ok(n) = self.shared_state.active_notes.lock() {
                    n.clone()
                } else {
                    std::collections::HashSet::new()
                };

                // Draw White Keys first
                let mut x_pos = rect.min.x;
                for note in 21..=108u8 {
                    let is_black = match note % 12 {
                        1 | 3 | 6 | 8 | 10 => true,
                        _ => false,
                    };
                    
                    if !is_black {
                        let active = active_set.contains(&note);
                        let color = if active { egui::Color32::GREEN } else { egui::Color32::WHITE };
                        
                        painter.rect_filled(
                            egui::Rect::from_min_size(egui::pos2(x_pos, rect.min.y), egui::vec2(white_key_width - 1.0, white_key_height)),
                            2.0,
                            color,
                        );
                        x_pos += white_key_width;
                    }
                }
                
                // Draw Black Keys on top
                // We need to re-iterate or track position properly. 
                // Easier to map note to x-offset.
                // A0 (21) is first white key.
                // White key index mapping:
                let mut white_key_idx = 0;
                for note in 21..=108u8 {
                    let is_black = match note % 12 {
                        1 | 3 | 6 | 8 | 10 => true,
                        _ => false,
                    };
                    
                    if is_black {
                         // Centered on the line between current white key index (which is actually previous white key) 
                         // and next.
                         // Current white_key_idx represents the number of white keys passed.
                         // The black key sits after the (white_key_idx - 1)-th key.
                         let center_x = rect.min.x + (white_key_idx as f32 * white_key_width);
                         
                         let active = active_set.contains(&note);
                         let color = if active { egui::Color32::GREEN } else { egui::Color32::BLACK };
                         
                         painter.rect_filled(
                            egui::Rect::from_min_size(
                                egui::pos2(center_x - (black_key_width / 2.0), rect.min.y), 
                                egui::vec2(black_key_width, black_key_height)
                            ),
                            2.0,
                            color,
                        );
                    } else {
                        white_key_idx += 1;
                    }
                }
            });
        });
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Force X11 backend to ensure Always On Top works
    unsafe { std::env::remove_var("WAYLAND_DISPLAY") };

    println!("Initializing virtual keyboard (requires permissions to write to /dev/uinput)...");
    
    // keys is a set of KeyCodes
    let mut keys = AttributeSet::<KeyCode>::new();
    keys.insert(KeyCode::KEY_E);
    keys.insert(KeyCode::KEY_LEFTSHIFT);
    keys.insert(KeyCode::KEY_LEFTCTRL);
    keys.insert(KeyCode::KEY_UP);
    keys.insert(KeyCode::KEY_DOWN);
    
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
    options.viewport = egui::ViewportBuilder::default().with_transparent(true);
    eframe::run_native(
        "Miditoroblox",
        options,
        Box::new(|cc| Ok(Box::new(MidiApp::new(cc, device)))),
    ).map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

    Ok(())
}
