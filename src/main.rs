use eframe::egui;
use evdev::{uinput::VirtualDevice, AttributeSet, EventType, InputEvent, KeyCode};
use midir::{MidiInput, MidiInputConnection, MidiInputPort};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{self, SystemTime, UNIX_EPOCH};
use std::thread;

mod solver;
use solver::{Solver, SolverMode};

// Mappings in solver.rs because yes

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
    // Solver Settings
    solver_enabled: AtomicBool,
    solver_mode_efficiency: AtomicBool, // true = Efficiency, false = Accuracy
    solver_max_jump: AtomicU64,
    transpose_range: AtomicU64,
    active_notes: Mutex<std::collections::HashSet<u8>>,
    // Keys actually held down (Visualizer output) - tracking specific keys / notes

    active_output_notes: Mutex<std::collections::HashSet<u8>>,
    
    visualizer_enabled: AtomicBool,
    visualizer_show_midi: AtomicBool,
    visualizer_show_roblox: AtomicBool,
    
    ui_context: Mutex<Option<egui::Context>>,
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
                solver_max_jump: AtomicU64::new(12),
                transpose_range: AtomicU64::new(24),
                active_notes: Mutex::new(std::collections::HashSet::new()),
                active_output_notes: Mutex::new(std::collections::HashSet::new()),
                visualizer_enabled: AtomicBool::new(true),
                visualizer_show_midi: AtomicBool::new(true),
                visualizer_show_roblox: AtomicBool::new(true),
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

        // Header Section (MIDI Selector & Window Settings)
        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.horizontal(|ui| {
                // MIDI Selector
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let ports_len = self.available_ports.len();
                    ui.label("Midi Device:");
                    let response = egui::ComboBox::from_id_source("midi_selector_header")
                        .selected_text(self.selected_port_name.as_deref().unwrap_or("Select MIDI Device"))
                        .show_ui(ui, |ui| {
                            for (i, (port_name, _)) in self.available_ports.iter().enumerate() {
                                if ui.selectable_value(&mut self.selected_port_name, Some(port_name.clone()), port_name).clicked() {
                                    // Handle selection if needed
                                }
                            }
                        });
                    
                    if ui.button("Refresh").clicked() {
                        self.refresh_ports();
                    }
                });

                // Window Settings (Opacity & Always On Top)
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                     // Always On Top
                    if ui.checkbox(&mut self.always_on_top, "Always On Top").changed() {
                        let level = if self.always_on_top {
                            egui::WindowLevel::AlwaysOnTop
                        } else {
                            egui::WindowLevel::Normal
                        };
                        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
                    }
                    
                    ui.add_space(10.0);

                    ui.label("Opacity:");
                    if ui.add(egui::Slider::new(&mut self.window_opacity, 0.1..=1.0).show_value(false)).changed() {
                        let mut visuals = egui::Visuals::dark();
                        let alpha = (self.window_opacity * 255.0) as u8;
                        visuals.window_fill = egui::Color32::from_black_alpha(alpha);
                        visuals.panel_fill = egui::Color32::from_black_alpha(alpha);
                        ctx.set_visuals(visuals);
                    }
                });
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {

            // Connection controls
            if let Some(_) = &self.connection {
                ui.horizontal(|ui| {
                     ui.label(egui::RichText::new("Status: Connected").color(egui::Color32::GREEN));
                     if ui.button("Disconnect").clicked() {
                         self.connection = None;
                         self.status_message = "Disconnected".to_string();
                         if self.midi_input.is_none() {
                             self.midi_input = Some(MidiInput::new("Miditoroblox Input").unwrap());
                         }
                         self.refresh_ports();
                     }
                });
                
                ui.separator();

                // Settings Group
                egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                    let mut base_enabled = self.shared_state.base_mapping_enabled.load(Ordering::Relaxed);
                    let mut low_enabled = self.shared_state.low_mapping_enabled.load(Ordering::Relaxed);
                    let mut high_enabled = self.shared_state.high_mapping_enabled.load(Ordering::Relaxed);

                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut base_enabled, "Start (Middle Octaves)").changed() {
                            self.shared_state.base_mapping_enabled.store(base_enabled, Ordering::Relaxed);
                        }
                        if ui.checkbox(&mut low_enabled, "Low Range").changed() {
                            self.shared_state.low_mapping_enabled.store(low_enabled, Ordering::Relaxed);
                        }
                        if ui.checkbox(&mut high_enabled, "High Range").changed() {
                            self.shared_state.high_mapping_enabled.store(high_enabled, Ordering::Relaxed);
                        }
                    });

                    let mut auto_transpose = self.shared_state.auto_transpose_enabled.load(Ordering::Relaxed);
                    if ui.checkbox(&mut auto_transpose, "Enable Auto-Octave Transposition").changed() {
                        self.shared_state.auto_transpose_enabled.store(auto_transpose, Ordering::Relaxed);
                    }

                    ui.separator();
                    
                    // Experimental Section
                    ui.label(egui::RichText::new("Experimental").strong());
                    
                    let mut exp_transpose = self.shared_state.experimental_transpose_enabled.load(Ordering::Relaxed);
                    if ui.checkbox(&mut exp_transpose, "Black Keys using Transpose").changed() {
                        self.shared_state.experimental_transpose_enabled.store(exp_transpose, Ordering::Relaxed);
                    }
                    
                    if exp_transpose {
                        let mut delay = self.shared_state.transpose_delay_ms.load(Ordering::Relaxed);
                        if ui.add(egui::Slider::new(&mut delay, 0..=1000).text("Transpose Delay (ms)")).changed() {
                            self.shared_state.transpose_delay_ms.store(delay, Ordering::Relaxed);
                        }
                        let mut lazy = self.shared_state.lazy_transpose_enabled.load(Ordering::Relaxed);
                        if ui.checkbox(&mut lazy, "Optimized Transpose").changed() {
                            self.shared_state.lazy_transpose_enabled.store(lazy, Ordering::Relaxed);
                        }
                    }

                    let mut exp_hold = self.shared_state.experimental_hold_ctrl_enabled.load(Ordering::Relaxed);
                    if ui.checkbox(&mut exp_hold, "Hold CTRL for Upper/Lower ranges").changed() {
                        self.shared_state.experimental_hold_ctrl_enabled.store(exp_hold, Ordering::Relaxed);
                    }

                    let mut solver_en = self.shared_state.solver_enabled.load(Ordering::Relaxed);
                    if ui.checkbox(&mut solver_en, "Smart Solver").changed() {
                        self.shared_state.solver_enabled.store(solver_en, Ordering::Relaxed);
                    }
                     
                    if solver_en {
                        ui.indent("solver_settings", |ui| {
                            let mut is_efficiency = self.shared_state.solver_mode_efficiency.load(Ordering::Relaxed);
                            ui.horizontal(|ui| {
                                if ui.radio_value(&mut is_efficiency, true, "Efficiency (Least Clicks)").clicked() {
                                    self.shared_state.solver_mode_efficiency.store(true, Ordering::Relaxed);
                                }
                                if ui.radio_value(&mut is_efficiency, false, "Accuracy (Best Match)").clicked() {
                                    self.shared_state.solver_mode_efficiency.store(false, Ordering::Relaxed);
                                }
                            });
                            
                            let mut max_jump = self.shared_state.solver_max_jump.load(Ordering::Relaxed);
                            if ui.add(egui::Slider::new(&mut max_jump, 1..=24).text("Max Jump Distance")).changed() {
                                self.shared_state.solver_max_jump.store(max_jump, Ordering::Relaxed);
                            }
                            
                            let mut range = self.shared_state.transpose_range.load(Ordering::Relaxed);
                            if ui.add(egui::Slider::new(&mut range, 12..=36).text("Transposition Range (+/-)")).changed() {
                                self.shared_state.transpose_range.store(range, Ordering::Relaxed);
                            }
                            
                            ui.horizontal(|ui| {
                                if ui.button("Reset Solver").clicked() {
                                     let mut state = self.shared_state.device_state.lock().unwrap();
                                     state.solver.reset_transpose();
                                     state.current_transpose_offset = 0;
                                }
                                if ui.button("Release Keys").clicked() {
                                    let mut state = self.shared_state.device_state.lock().unwrap();
                                    let keys = state.solver.reset_keys();
                                    for k in keys {
                                        let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, k.code(), 0)]);
                                    }
                                    let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 0)]);
                                    let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 0)]);
                                }
                            });
                        });
                    }

                    ui.separator();
                    
                    // Quantization
                    let mut quant_enabled = self.shared_state.quantize_enabled.load(Ordering::Relaxed);
                    if ui.checkbox(&mut quant_enabled, "Enable Note Quantization").changed() {
                        self.shared_state.quantize_enabled.store(quant_enabled, Ordering::Relaxed);
                    }
                    if quant_enabled {
                        let mut ms = self.shared_state.quantize_ms.load(Ordering::Relaxed);
                        if ui.add(egui::Slider::new(&mut ms, 10..=500).text("Quantize (ms)")).changed() {
                            self.shared_state.quantize_ms.store(ms, Ordering::Relaxed);
                        }
                    }
                });
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
                                     if message.len() < 3 { return; }
                                     let status = message[0] & 0xF0;
                                     let channel = message[0] & 0x0F;
                                     let note_original = message[1];
                                     let velocity = message[2];

                                     // Update Visualizer State (Input)
                                     if status == 0x90 && velocity > 0 {
                                         if let Ok(mut notes) = shared_state.active_notes.lock() {
                                             notes.insert(note_original);
                                         }
                                         // Real output tracking happens below when we emit keys.
                                         
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
                                         // Note Off Repaint
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
                                     
                                     // Validate Note

                                     
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
                                     
                                     let use_solver = shared_state.solver_enabled.load(Ordering::Relaxed);

                                     if !use_solver {
                                          if !valid && shared_state.auto_transpose_enabled.load(Ordering::Relaxed) {
                                              // Auto-transpose up
                                              let mut test_note = final_note;
                                              while test_note <= 108 && !is_note_valid(test_note) {
                                                   if let Some(next) = test_note.checked_add(12) { test_note = next; } else { break; }
                                              }
                                              if is_note_valid(test_note) { final_note = test_note; valid = true; } 
                                              else {
                                                   // Auto-transpose down
                                                   let mut test_note = final_note;
                                                   while test_note >= 21 && !is_note_valid(test_note) {
                                                       if let Some(prev) = test_note.checked_sub(12) { test_note = prev; } else { break; }
                                                   }
                                                   if is_note_valid(test_note) { final_note = test_note; valid = true; }
                                              }
                                          }
    
                                          if !valid { return; }
                                     }
                                     
                                     // Quantization
                                     if status == 0x90 && velocity > 0 && shared_state.quantize_enabled.load(Ordering::Relaxed) {
                                          let grid = shared_state.quantize_ms.load(Ordering::Relaxed);
                                          if grid > 0 {
                                              if let Ok(duration) = SystemTime::now().duration_since(UNIX_EPOCH) {
                                                   let rem = (duration.as_millis() as u64) % grid;
                                                   if rem > 0 {
                                                       thread::sleep(time::Duration::from_millis(grid - rem));
                                                   }
                                              }
                                          }
                                     }
                                     
                                     if use_solver {
                                         let mut state = shared_state.device_state.lock().unwrap();
                                         if status == 0x90 && velocity > 0 {
                                             let mode = if shared_state.solver_mode_efficiency.load(Ordering::Relaxed) { SolverMode::Efficiency } else { SolverMode::Accuracy };
                                             let max_jump = shared_state.solver_max_jump.load(Ordering::Relaxed) as i32;
                                             let range = shared_state.transpose_range.load(Ordering::Relaxed) as i32;
                                             
                                             if let Some((delta, mapping)) = state.solver.solve(note_original, mode, max_jump, range) {
                                                 // Track Output
                                                 if let Ok(mut out_notes) = shared_state.active_output_notes.lock() {
                                                     out_notes.insert(note_original);
                                                 }

                                                 // Adjust Transpose
                                                 let current = state.solver.current_transpose;
                                                 if delta != current {
                                                     let diff = delta - current;
                                                     let key = if diff > 0 { KeyCode::KEY_UP } else { KeyCode::KEY_DOWN };
                                                     for _ in 0..diff.abs() {
                                                         let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, key.code(), 1)]);
                                                         let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, key.code(), 0)]);
                                                         thread::sleep(time::Duration::from_millis(5));
                                                     }
                                                     state.current_transpose_offset = delta;
                                                 }
                                                 
                                                 // Press Note
                                                 // Handle Active Key "Stealing"
                                                 // The solver now allows returning a busy key with a penalty.
                                                 // Check if key is physically held?
                                                 // state.solver.active_keys tracks keys with active notes.
                                                 if state.solver.active_keys.contains_key(&mapping.key_code) && !state.solver.active_keys[&mapping.key_code].is_empty() {
                                                      // Force Release first
                                                      let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, mapping.key_code.code(), 0)]);
                                                      thread::sleep(time::Duration::from_millis(5)); // Brief pause
                                                 }

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
                                                 
                                                 let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, mapping.key_code.code(), 1)]);
                                                 state.solver.register_note_on(mapping.key_code, note_original, delta, mapping.shift, mapping.ctrl);
                                             }
                                         } else if status == 0x80 || (status == 0x90 && velocity == 0) {
                                             if let Some(key) = state.solver.register_note_off(note_original) {
                                                 // Track Output Removel
                                                 if let Ok(mut out_notes) = shared_state.active_output_notes.lock() {
                                                     out_notes.remove(&note_original);
                                                 }

                                                 let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, key.code(), 0)]);
                                                 
                                                 // Modifiers cleanup
                                                 if !state.solver.shift_active {
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 0)]);
                                                 }
                                                 if !state.solver.ctrl_active {
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 0)]);
                                                 }
                                             }
                                         }
                                         return;
                                     }

                                     // Legacy Logic
                                     let use_experimental_transpose = shared_state.experimental_transpose_enabled.load(Ordering::Relaxed);
                                     let use_hold_ctrl = shared_state.experimental_hold_ctrl_enabled.load(Ordering::Relaxed);

                                     let mappings = solver::get_available_mappings();
                                     if let Some(mapping) = mappings.iter().find(|m| m.midi_note == final_note) {
                                         let mut state = shared_state.device_state.lock().unwrap();
                                         let mapping_code = mapping.key_code;
                                         let mapping_shift = mapping.shift;
                                         let mapping_ctrl = mapping.ctrl;
                                         
                                         if status == 0x90 && velocity > 0 {
                                             if let Ok(mut out_notes) = shared_state.active_output_notes.lock() { out_notes.insert(note_original); }
                                             
                                             let mut handled_transpose = false;
                                             
                                             if use_experimental_transpose {
                                                 let use_lazy = shared_state.lazy_transpose_enabled.load(Ordering::Relaxed);
                                                 if use_lazy {
                                                     let target_offset = if mapping_shift && !mapping_ctrl { 1 } else { 0 };
                                                     let current_offset = state.current_transpose_offset;
                                                     if target_offset != current_offset {
                                                         let delay_ms = shared_state.transpose_delay_ms.load(Ordering::Relaxed);
                                                         if target_offset > current_offset {
                                                             let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_UP.code(), 1)]);
                                                             let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_UP.code(), 0)]);
                                                         } else {
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
                                                     state.current_transpose_offset = 0; 
                                                 }
                                             }
 
                                             if mapping_ctrl {
                                                 if use_hold_ctrl {
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 1)]);
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, mapping_code.code(), 1)]);
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 0)]);
                                                 } else {
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 1)]);
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, mapping_code.code(), 1)]);
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, mapping_code.code(), 0)]);
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 0)]);
                                                 }
                                             } else if mapping_shift {
                                                 if use_experimental_transpose {
                                                     if handled_transpose {
                                                         let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, mapping_code.code(), 1)]);
                                                     } else {
                                                         let delay_ms = shared_state.transpose_delay_ms.load(Ordering::Relaxed);
                                                         let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_UP.code(), 1)]);
                                                         let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_UP.code(), 0)]);
                                                         if delay_ms > 0 { drop(state); thread::sleep(time::Duration::from_millis(delay_ms)); state = shared_state.device_state.lock().unwrap(); }
                                                         let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, mapping_code.code(), 1)]);
                                                         if delay_ms > 0 { drop(state); thread::sleep(time::Duration::from_millis(delay_ms)); state = shared_state.device_state.lock().unwrap(); }
                                                         let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_DOWN.code(), 1)]);
                                                         let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_DOWN.code(), 0)]);
                                                     }
                                                 } else {
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 1)]);
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, mapping_code.code(), 1)]);
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, mapping_code.code(), 0)]);
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 0)]);
                                                 }
                                             } else {
                                                  let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, mapping_code.code(), 1)]);
                                             }
                                         }
                                         else if status == 0x80 || (status == 0x90 && velocity == 0) {
                                              if let Ok(mut out_notes) = shared_state.active_output_notes.lock() { out_notes.remove(&note_original); }

                                              if mapping_ctrl && use_hold_ctrl {
                                                  let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, mapping_code.code(), 0)]);
                                              } else if mapping_shift && use_experimental_transpose {
                                                  let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, mapping_code.code(), 0)]);
                                              } else if !mapping_shift && !mapping_ctrl {
                                                  let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, mapping_code.code(), 0)]);
                                              }
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
            ui.label(format!("Log: {}", self.status_message));
            
            ui.add_space(10.0);
            ui.separator();
            
            let mut vis_enabled = self.shared_state.visualizer_enabled.load(Ordering::Relaxed);
            ui.horizontal(|ui| {
                if ui.checkbox(&mut vis_enabled, "Show Visualizer").changed() {
                     self.shared_state.visualizer_enabled.store(vis_enabled, Ordering::Relaxed);
                }
                
                if vis_enabled {
                    ui.separator();
                    ui.label("Show Mode:");
                    egui::ComboBox::from_id_source("vis_mode")
                        .selected_text("Select Modes...")
                        .show_ui(ui, |ui| {
                             let mut show_midi = self.shared_state.visualizer_show_midi.load(Ordering::Relaxed);
                             if ui.checkbox(&mut show_midi, "Midi Inputs").changed() {
                                 self.shared_state.visualizer_show_midi.store(show_midi, Ordering::Relaxed);
                             }
                             let mut show_roblox = self.shared_state.visualizer_show_roblox.load(Ordering::Relaxed);
                             if ui.checkbox(&mut show_roblox, "Roblox Played").changed() {
                                 self.shared_state.visualizer_show_roblox.store(show_roblox, Ordering::Relaxed);
                             }
                        });
                }
            });
            
            if vis_enabled {
                egui::ScrollArea::horizontal().enable_scrolling(false).show(ui, |ui| {
                    let (response, painter) = ui.allocate_painter(egui::vec2(ui.available_width(), 100.0), egui::Sense::hover());
                    let rect = response.rect;
                    
                    let white_key_width = rect.width() / 52.0; 
                    let black_key_width = white_key_width * 0.6;
                    let white_key_height = rect.height();
                    let black_key_height = rect.height() * 0.6;
                    
                    let input_set = if let Ok(n) = self.shared_state.active_notes.lock() { n.clone() } else { std::collections::HashSet::new() };
                    let output_set = if let Ok(n) = self.shared_state.active_output_notes.lock() { n.clone() } else { std::collections::HashSet::new() };
                    
                    let show_input = self.shared_state.visualizer_show_midi.load(Ordering::Relaxed);
                    let show_output = self.shared_state.visualizer_show_roblox.load(Ordering::Relaxed);

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
                        } else if inp {
                             painter.rect_filled(key_rect, if is_black {1.0} else {2.0}, input_color);
                        } else if outp {
                             painter.rect_filled(key_rect, if is_black {1.0} else {2.0}, output_color);
                        } else {
                             painter.rect_filled(key_rect, if is_black {1.0} else {2.0}, base_color);
                        }
                        painter.rect(key_rect, 1.0, egui::Color32::TRANSPARENT, egui::Stroke::new(1.0, egui::Color32::GRAY), egui::StrokeKind::Inside);
                    };

                    let mut x_pos = rect.min.x;
                    for note in 21..=108u8 {
                         let is_black = match note % 12 { 1 | 3 | 6 | 8 | 10 => true, _ => false };
                         if !is_black {
                             let key_rect = egui::Rect::from_min_size(egui::pos2(x_pos, rect.min.y), egui::vec2(white_key_width, white_key_height));
                             draw_key(key_rect, note, false);
                             x_pos += white_key_width;
                         }
                    }
                    
                    let mut white_key_idx = 0;
                    for note in 21..=108u8 {
                        let is_black = match note % 12 { 1 | 3 | 6 | 8 | 10 => true, _ => false };
                        if is_black {
                             let center_x = rect.min.x + (white_key_idx as f32 * white_key_width);
                             let key_rect = egui::Rect::from_min_size(egui::pos2(center_x - (black_key_width/2.0), rect.min.y), egui::vec2(black_key_width, black_key_height));
                             draw_key(key_rect, note, true);
                        } else {
                            white_key_idx += 1;
                        }
                    }
                });
            }
        });
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Force X11 backend to ensure Always On Top works
    unsafe { std::env::remove_var("WAYLAND_DISPLAY") };

    println!("Initializing virtual keyboard (requires permissions to write to /dev/uinput)...");
    
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
