use eframe::egui;
use evdev::{uinput::VirtualDevice, AttributeSet, Device, EventType, InputEvent, KeyCode};
use midir::{MidiInput, MidiInputConnection, MidiInputPort};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
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
    use_midi_device: bool,
    show_midi_window: bool,
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
            }),
            status_message: "Ready".to_string(),
            window_opacity: 1.0,
            always_on_top: false,
            use_midi_device: false,
            show_midi_window: false,
        };
        
        // Initialize visuals (opaque default)
        let mut visuals = egui::Visuals::dark();
        visuals.window_fill = egui::Color32::from_black_alpha(255);
        visuals.panel_fill = egui::Color32::from_black_alpha(255);
        cc.egui_ctx.set_visuals(visuals);

        app.refresh_ports();
        app.spawn_panic_monitor();
        app
    }

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
                                                        let keys = state.solver.reset_keys();
                                                        for k in keys {
                                                            let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, k.code(), 0)]);
                                                        }
                                                        let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 0)]);
                                                        let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 0)]);
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

        // Top panel
        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Miditoroblox");
                ui.add_space(20.0);
                
                // Left Side
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut self.use_midi_device, "MIDI Device").changed() && !self.use_midi_device {
                        self.connection = None;
                        self.status_message = "Midi device disabled".to_string();
                    }
                    
                    if self.use_midi_device {
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
                ui.label(egui::RichText::new(format!("In-Game Transpose: {}", transpose)).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Reset All").clicked() {
                        if let Ok(mut state) = self.shared_state.device_state.lock() {
                            state.solver.reset_transpose();
                            state.current_transpose_offset = 0;
                            let keys = state.solver.reset_keys();
                            for k in keys { let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, k.code(), 0)]); }
                            let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 0)]);
                            let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 0)]);
                        }
                        if let Ok(mut notes) = self.shared_state.active_notes.lock() { notes.clear(); }
                        if let Ok(mut notes) = self.shared_state.active_output_notes.lock() { notes.clear(); }
                    }
                    ui.add_space(10.0);
                    if ui.button("Release All").clicked() {
                        if let Ok(mut state) = self.shared_state.device_state.lock() {
                            let keys = state.solver.reset_keys();
                            for k in keys { let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, k.code(), 0)]); }
                            let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 0)]);
                            let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 0)]);
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
            if self.use_midi_device {
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
                                         if shared_state.panic_active.load(Ordering::Relaxed) { return; }
                                         if message.len() < 3 { return; }
                                         let (status, channel, note_original, velocity) = (message[0] & 0xF0, message[0] & 0x0F, message[1], message[2]);

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
                                                      let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_APOSTROPHE.code(), 1)]);
                                                      let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_APOSTROPHE.code(), 0)]);
                                                      shared_state.sustain_active.store(pedal_down, Ordering::Relaxed);
                                                  }
                                              }
                                              return;
                                         }

                                         if channel == 9 { return; }
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
                                                      let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTALT.code(), 1)]);
                                                      if delay > 0 { thread::sleep(time::Duration::from_millis(delay)); }
                                                      let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, target_key, 1)]);
                                                      if delay > 0 { thread::sleep(time::Duration::from_millis(delay)); }
                                                      let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, target_key, 0)]);
                                                      if delay > 0 { thread::sleep(time::Duration::from_millis(delay)); }
                                                      let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTALT.code(), 0)]);
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
                                                 handle_velocity(&mut state, velocity, shared_state);
                                                 let (mode, max_j, range) = (if shared_state.solver_mode_efficiency.load(Ordering::Relaxed) { SolverMode::Efficiency } else { SolverMode::Accuracy }, shared_state.solver_max_jump.load(Ordering::Relaxed) as i32, shared_state.transpose_range.load(Ordering::Relaxed) as i32);
                                                 if let Some((delta, m)) = state.solver.solve(note_original, mode, max_j, range) {
                                                     if let Ok(mut out) = shared_state.active_output_notes.lock() { out.insert(note_original); }
                                                     let curr = state.solver.current_transpose;
                                                     if delta != curr {
                                                         let diff = delta - curr;
                                                         let key = if diff > 0 { KeyCode::KEY_UP } else { KeyCode::KEY_DOWN };
                                                         for _ in 0..diff.abs() { let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, key.code(), 1)]); let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, key.code(), 0)]); thread::sleep(time::Duration::from_millis(5)); }
                                                         state.current_transpose_offset = delta;
                                                     }
                                                     if state.solver.active_keys.contains_key(&m.key_code) { let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, m.key_code.code(), 0)]); thread::sleep(time::Duration::from_millis(5)); }
                                                     if m.shift && !state.solver.shift_active { let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 1)]); } else if !m.shift && state.solver.shift_active { let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 0)]); }
                                                     if m.ctrl && !state.solver.ctrl_active { let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 1)]); } else if !m.ctrl && state.solver.ctrl_active { let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 0)]); }
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, m.key_code.code(), 1)]);
                                                     state.solver.register_note_on(m.key_code, note_original, delta, m.shift, m.ctrl);
                                                 }
                                             } else {
                                                 if let Some(key) = state.solver.register_note_off(note_original) {
                                                     if let Ok(mut out) = shared_state.active_output_notes.lock() { out.remove(&note_original); }
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, key.code(), 0)]);
                                                     if !state.solver.shift_active { let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 0)]); }
                                                     if !state.solver.ctrl_active { let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 0)]); }
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
                                                             let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, k.code(), 1)]); let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, k.code(), 0)]);
                                                             if d > 0 { drop(state); thread::sleep(time::Duration::from_millis(d)); state = shared_state.device_state.lock().unwrap(); }
                                                             state.current_transpose_offset = target;
                                                         }
                                                         handled_t = true;
                                                     } else { state.current_transpose_offset = 0; }
                                                 }
                                                 if m.ctrl {
                                                     if hold_ctrl { let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 1)]); let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, m.key_code.code(), 1)]); let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 0)]); }
                                                     else { let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 1)]); let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, m.key_code.code(), 1)]); let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, m.key_code.code(), 0)]); let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 0)]); }
                                                 } else if m.shift {
                                                     if experimental {
                                                         if handled_t { let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, m.key_code.code(), 1)]); }
                                                         else {
                                                             let d = shared_state.transpose_delay_ms.load(Ordering::Relaxed);
                                                             let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_UP.code(), 1)]); let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_UP.code(), 0)]);
                                                             if d > 0 { drop(state); thread::sleep(time::Duration::from_millis(d)); state = shared_state.device_state.lock().unwrap(); }
                                                             let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, m.key_code.code(), 1)]);
                                                             if d > 0 { drop(state); thread::sleep(time::Duration::from_millis(d)); state = shared_state.device_state.lock().unwrap(); }
                                                             let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_DOWN.code(), 1)]); let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_DOWN.code(), 0)]);
                                                         }
                                                     } else { let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 1)]); let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, m.key_code.code(), 1)]); let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, m.key_code.code(), 0)]); let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 0)]); }
                                                 } else { let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, m.key_code.code(), 1)]); }
                                             } else {
                                                  if let Ok(mut out) = shared_state.active_output_notes.lock() { out.remove(&note_original); }
                                                  if m.ctrl && hold_ctrl { let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, m.key_code.code(), 0)]); }
                                                  else if m.shift && experimental { let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, m.key_code.code(), 0)]); }
                                                  else if !m.shift && !m.ctrl { let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, m.key_code.code(), 0)]); }
                                             }
                                         }
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
            if self.connection.is_some() || !self.use_midi_device {
                egui::ScrollArea::vertical().max_height(ui.available_height()).show(ui, |ui| {
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

                    ui.separator();
                    ui.label(egui::RichText::new("Experimental").strong());
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

fn is_velocity_key(code: u16) -> bool {
    match code {
        c if c >= 2 && c <= 11 => true,  // 1-0
        c if c >= 16 && c <= 25 => true, // Q-P
        c if c >= 30 && c <= 38 => true, // A-L
        c if c == 44 || c == 45 || c == 46 => true, // Z, X, C
        _ => false,
    }
}

const VELOCITY_KEYS: [u16; 32] = [
    2, 3, 4, 5, 6, 7, 8, 9, 10, 11, // 1-0
    16, 17, 18, 19, 20, 21, 22, 23, 24, 25, // Q-P
    30, 31, 32, 33, 34, 35, 36, 37, 38, // A-L
    44, 45, 46 // Z, X, C
];
