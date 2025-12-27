use eframe::egui;
use evdev::{uinput::VirtualDevice, AttributeSet, EventType, InputEvent, KeyCode};
use midir::{MidiInput, MidiInputConnection, MidiInputPort};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::{thread, time};

struct KeyMapping {
    midi_note: u8,
    key_code: KeyCode,
    shift: bool,
    ctrl: bool,
}

struct DeviceState {
    device: VirtualDevice,
}

struct SharedState {
    device_state: Mutex<DeviceState>,
    base_mapping_enabled: AtomicBool,
    low_mapping_enabled: AtomicBool,
    high_mapping_enabled: AtomicBool,
    auto_transpose_enabled: AtomicBool,
    experimental_transpose_enabled: AtomicBool,
}

fn get_mappings() -> Vec<KeyMapping> {
    vec![
        // --- Low Range (C1 to B1) - [CTRL] ---
        KeyMapping { midi_note: 24, key_code: KeyCode::KEY_1, shift: false, ctrl: true }, // C1  -> 1
        KeyMapping { midi_note: 25, key_code: KeyCode::KEY_2, shift: false, ctrl: true }, // C#1 -> 2
        KeyMapping { midi_note: 26, key_code: KeyCode::KEY_3, shift: false, ctrl: true }, // D1  -> 3
        KeyMapping { midi_note: 27, key_code: KeyCode::KEY_4, shift: false, ctrl: true }, // D#1 -> 4
        KeyMapping { midi_note: 28, key_code: KeyCode::KEY_5, shift: false, ctrl: true }, // E1  -> 5
        KeyMapping { midi_note: 29, key_code: KeyCode::KEY_6, shift: false, ctrl: true }, // F1  -> 6
        KeyMapping { midi_note: 30, key_code: KeyCode::KEY_7, shift: false, ctrl: true }, // F#1 -> 7
        KeyMapping { midi_note: 31, key_code: KeyCode::KEY_8, shift: false, ctrl: true }, // G1  -> 8
        KeyMapping { midi_note: 32, key_code: KeyCode::KEY_9, shift: false, ctrl: true }, // G#1 -> 9
        KeyMapping { midi_note: 33, key_code: KeyCode::KEY_E, shift: false, ctrl: true }, // A1  -> e
        KeyMapping { midi_note: 34, key_code: KeyCode::KEY_R, shift: false, ctrl: true }, // A#1 -> r
        KeyMapping { midi_note: 35, key_code: KeyCode::KEY_T, shift: false, ctrl: true }, // B1  -> t

        // --- Lower Octaves (C2 to B3) ---
        KeyMapping { midi_note: 36, key_code: KeyCode::KEY_1, shift: false, ctrl: false }, // C2
        KeyMapping { midi_note: 37, key_code: KeyCode::KEY_1, shift: true,  ctrl: false }, // C#2 -> !
        KeyMapping { midi_note: 38, key_code: KeyCode::KEY_2, shift: false, ctrl: false }, // D2
        KeyMapping { midi_note: 39, key_code: KeyCode::KEY_2, shift: true,  ctrl: false }, // D#2 -> @
        KeyMapping { midi_note: 40, key_code: KeyCode::KEY_3, shift: false, ctrl: false }, // E2
        KeyMapping { midi_note: 41, key_code: KeyCode::KEY_4, shift: false, ctrl: false }, // F2
        KeyMapping { midi_note: 42, key_code: KeyCode::KEY_4, shift: true,  ctrl: false }, // F#2 -> $
        KeyMapping { midi_note: 43, key_code: KeyCode::KEY_5, shift: false, ctrl: false }, // G2
        KeyMapping { midi_note: 44, key_code: KeyCode::KEY_5, shift: true,  ctrl: false }, // G#2 -> %
        KeyMapping { midi_note: 45, key_code: KeyCode::KEY_6, shift: false, ctrl: false }, // A2
        KeyMapping { midi_note: 46, key_code: KeyCode::KEY_6, shift: true,  ctrl: false }, // A#2 -> ^
        KeyMapping { midi_note: 47, key_code: KeyCode::KEY_7, shift: false, ctrl: false }, // B2
        KeyMapping { midi_note: 48, key_code: KeyCode::KEY_8, shift: false, ctrl: false }, // C3
        KeyMapping { midi_note: 49, key_code: KeyCode::KEY_8, shift: true,  ctrl: false }, // C#3 -> *
        KeyMapping { midi_note: 50, key_code: KeyCode::KEY_9, shift: false, ctrl: false }, // D3
        KeyMapping { midi_note: 51, key_code: KeyCode::KEY_9, shift: true,  ctrl: false }, // D#3 -> (
        KeyMapping { midi_note: 52, key_code: KeyCode::KEY_0, shift: false, ctrl: false }, // E3
        KeyMapping { midi_note: 53, key_code: KeyCode::KEY_Q, shift: false, ctrl: false }, // F3
        KeyMapping { midi_note: 54, key_code: KeyCode::KEY_Q, shift: true,  ctrl: false }, // F#3 -> Q
        KeyMapping { midi_note: 55, key_code: KeyCode::KEY_W, shift: false, ctrl: false }, // G3
        KeyMapping { midi_note: 56, key_code: KeyCode::KEY_W, shift: true,  ctrl: false }, // G#3 -> W
        KeyMapping { midi_note: 57, key_code: KeyCode::KEY_E, shift: false, ctrl: false }, // A3
        KeyMapping { midi_note: 58, key_code: KeyCode::KEY_E, shift: true,  ctrl: false }, // A#3 -> E
        KeyMapping { midi_note: 59, key_code: KeyCode::KEY_R, shift: false, ctrl: false }, // B3

        // --- Middle Octaves (C4 to C6) ---
        KeyMapping { midi_note: 60, key_code: KeyCode::KEY_T, shift: false, ctrl: false }, // C4
        KeyMapping { midi_note: 61, key_code: KeyCode::KEY_T, shift: true,  ctrl: false }, // C#4 -> T
        KeyMapping { midi_note: 62, key_code: KeyCode::KEY_Y, shift: false, ctrl: false }, // D4
        KeyMapping { midi_note: 63, key_code: KeyCode::KEY_Y, shift: true,  ctrl: false }, // D#4 -> Y
        KeyMapping { midi_note: 64, key_code: KeyCode::KEY_U, shift: false, ctrl: false }, // E4
        KeyMapping { midi_note: 65, key_code: KeyCode::KEY_I, shift: false, ctrl: false }, // F4
        KeyMapping { midi_note: 66, key_code: KeyCode::KEY_I, shift: true,  ctrl: false }, // F#4 -> I
        KeyMapping { midi_note: 67, key_code: KeyCode::KEY_O, shift: false, ctrl: false }, // G4
        KeyMapping { midi_note: 68, key_code: KeyCode::KEY_O, shift: true,  ctrl: false }, // G#4 -> O
        KeyMapping { midi_note: 69, key_code: KeyCode::KEY_P, shift: false, ctrl: false }, // A4
        KeyMapping { midi_note: 70, key_code: KeyCode::KEY_P, shift: true,  ctrl: false }, // A#4 -> P
        KeyMapping { midi_note: 71, key_code: KeyCode::KEY_A, shift: false, ctrl: false }, // B4
        KeyMapping { midi_note: 72, key_code: KeyCode::KEY_S, shift: false, ctrl: false }, // C5
        KeyMapping { midi_note: 73, key_code: KeyCode::KEY_S, shift: true,  ctrl: false }, // C#5 -> S
        KeyMapping { midi_note: 74, key_code: KeyCode::KEY_D, shift: false, ctrl: false }, // D5
        KeyMapping { midi_note: 75, key_code: KeyCode::KEY_D, shift: true,  ctrl: false }, // D#5 -> D
        KeyMapping { midi_note: 76, key_code: KeyCode::KEY_F, shift: false, ctrl: false }, // E5
        KeyMapping { midi_note: 77, key_code: KeyCode::KEY_G, shift: false, ctrl: false }, // F5
        KeyMapping { midi_note: 78, key_code: KeyCode::KEY_G, shift: true,  ctrl: false }, // F#5 -> G
        KeyMapping { midi_note: 79, key_code: KeyCode::KEY_H, shift: false, ctrl: false }, // G5
        KeyMapping { midi_note: 80, key_code: KeyCode::KEY_H, shift: true,  ctrl: false }, // G#5 -> H
        KeyMapping { midi_note: 81, key_code: KeyCode::KEY_J, shift: false, ctrl: false }, // A5
        KeyMapping { midi_note: 82, key_code: KeyCode::KEY_J, shift: true,  ctrl: false }, // A#5 -> J
        KeyMapping { midi_note: 83, key_code: KeyCode::KEY_K, shift: false, ctrl: false }, // B5
        KeyMapping { midi_note: 84, key_code: KeyCode::KEY_L, shift: false, ctrl: false }, // C6
        KeyMapping { midi_note: 85, key_code: KeyCode::KEY_L, shift: true,  ctrl: false }, // C#6 -> L

        // --- High Octaves (D6 to C7) ---
        KeyMapping { midi_note: 86, key_code: KeyCode::KEY_Z, shift: false, ctrl: false }, // D6
        KeyMapping { midi_note: 87, key_code: KeyCode::KEY_Z, shift: true,  ctrl: false }, // D#6 -> Z
        KeyMapping { midi_note: 88, key_code: KeyCode::KEY_X, shift: false, ctrl: false }, // E6
        KeyMapping { midi_note: 89, key_code: KeyCode::KEY_C, shift: false, ctrl: false }, // F6
        KeyMapping { midi_note: 90, key_code: KeyCode::KEY_C, shift: true,  ctrl: false }, // F#6 -> C
        KeyMapping { midi_note: 91, key_code: KeyCode::KEY_V, shift: false, ctrl: false }, // G6
        KeyMapping { midi_note: 92, key_code: KeyCode::KEY_V, shift: true,  ctrl: false }, // G#6 -> V
        KeyMapping { midi_note: 93, key_code: KeyCode::KEY_B, shift: false, ctrl: false }, // A6
        KeyMapping { midi_note: 94, key_code: KeyCode::KEY_B, shift: true,  ctrl: false }, // A#6 -> B
        KeyMapping { midi_note: 95, key_code: KeyCode::KEY_N, shift: false, ctrl: false }, // B6
        KeyMapping { midi_note: 96, key_code: KeyCode::KEY_M, shift: false, ctrl: false }, // C7

        // --- High Range (C#7 to C8) - [CTRL] ---
        KeyMapping { midi_note: 97,  key_code: KeyCode::KEY_Y, shift: false, ctrl: true }, // C#7 -> y
        KeyMapping { midi_note: 98,  key_code: KeyCode::KEY_U, shift: false, ctrl: true }, // D7  -> u
        KeyMapping { midi_note: 99,  key_code: KeyCode::KEY_I, shift: false, ctrl: true }, // D#7 -> i
        KeyMapping { midi_note: 100, key_code: KeyCode::KEY_O, shift: false, ctrl: true }, // E7  -> o
        KeyMapping { midi_note: 101, key_code: KeyCode::KEY_P, shift: false, ctrl: true }, // F7  -> p
        KeyMapping { midi_note: 102, key_code: KeyCode::KEY_A, shift: false, ctrl: true }, // F#7 -> a
        KeyMapping { midi_note: 103, key_code: KeyCode::KEY_S, shift: false, ctrl: true }, // G7  -> s
        KeyMapping { midi_note: 104, key_code: KeyCode::KEY_D, shift: false, ctrl: true }, // G#7 -> d
        KeyMapping { midi_note: 105, key_code: KeyCode::KEY_F, shift: false, ctrl: true }, // A7  -> f
        KeyMapping { midi_note: 106, key_code: KeyCode::KEY_G, shift: false, ctrl: true }, // A#7 -> g
        KeyMapping { midi_note: 107, key_code: KeyCode::KEY_H, shift: false, ctrl: true }, // B7  -> h
        KeyMapping { midi_note: 108, key_code: KeyCode::KEY_J, shift: false, ctrl: true }, // C8  -> j
    ]
}


struct MidiApp {
    midi_input: Option<MidiInput>,
    available_ports: Vec<(String, MidiInputPort)>,
    selected_port_name: Option<String>,
    connection: Option<MidiInputConnection<Arc<SharedState>>>,
    shared_state: Arc<SharedState>,
    status_message: String,
}

impl MidiApp {
    fn new(_cc: &eframe::CreationContext<'_>, virtual_device: VirtualDevice) -> Self {
        let mut app = Self {
            midi_input: Some(MidiInput::new("Miditoroblox Input").unwrap()),
            available_ports: Vec::new(),
            selected_port_name: None,
            connection: None,
            shared_state: Arc::new(SharedState {
                device_state: Mutex::new(DeviceState {
                    device: virtual_device,
                }),
                base_mapping_enabled: AtomicBool::new(false),
                low_mapping_enabled: AtomicBool::new(false),
                high_mapping_enabled: AtomicBool::new(false),
                auto_transpose_enabled: AtomicBool::new(false),
                experimental_transpose_enabled: AtomicBool::new(false),
            }),
            status_message: "Ready".to_string(),
        };
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
                if ui.checkbox(&mut low_enabled, "Enable Low Range (C1-C#1...)").changed() {
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
                                     
                                     // Auto-transpose logic
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
                                              while test_note >= 24 && !is_note_valid(test_note) {
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
                                     
                                     let use_experimental_transpose = shared_state.experimental_transpose_enabled.load(Ordering::Relaxed);

                                     let mappings = get_mappings();
                                     if let Some(mapping) = mappings.iter().find(|m| m.midi_note == final_note) {
                                         let mut state = shared_state.device_state.lock().unwrap();
                                         let mapping_code = mapping.key_code;
                                         let mapping_shift = mapping.shift;
                                         let mapping_ctrl = mapping.ctrl;
                                         
                                         // Note On (and velocity > 0)
                                         if status == 0x90 && velocity > 0 {
                                             if mapping_ctrl {
                                                 // Ctrl Key: Atomic Ctrl Tap
                                                 let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 1)]);
                                                 let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, mapping_code.code(), 1)]);
                                                 let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, mapping_code.code(), 0)]);
                                                 let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 0)]);
                                             } else if mapping_shift {
                                                 if use_experimental_transpose {
                                                     // Experimental Transpose Method
                                                     // 1. Up Arrow Tap (Transpose +1)
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_UP.code(), 1)]);
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_UP.code(), 0)]);
                                                     // 2. Key Down (Hold)
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, mapping_code.code(), 1)]);
                                                     // 3. Down Arrow Tap (Transpose -1 aka Back to 0)
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_DOWN.code(), 1)]);
                                                     let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_DOWN.code(), 0)]);
                                                     
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
                                              if mapping_shift && use_experimental_transpose {
                                                  // Release logic for experimental mode: Stop Holding Key
                                                  let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, mapping_code.code(), 0)]);
                                              } else if !mapping_shift && !mapping_ctrl {
                                                  // White Key: Normal Release
                                                  let _ = state.device.emit(&[InputEvent::new(EventType::KEY.0, mapping_code.code(), 0)]);
                                              }
                                              // Standard Black/Ctrl keys are auto-released on NoteOn, so ignore NoteOff
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
        });
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Initializing virtual keyboard (requires permissions to write to /dev/uinput)...");
    
    // keys is a set of KeyCodes
    let mut keys = AttributeSet::<KeyCode>::new();
    keys.insert(KeyCode::KEY_E);
    keys.insert(KeyCode::KEY_LEFTSHIFT);
    keys.insert(KeyCode::KEY_LEFTCTRL);
    keys.insert(KeyCode::KEY_UP);
    keys.insert(KeyCode::KEY_DOWN);
    
    // Register all mapped keys
    for mapping in get_mappings() {
        keys.insert(mapping.key_code);
    }

    // Create the virtual device using the builder
    let device = VirtualDevice::builder()?
        .name("Miditoroblox Rust Presser")
        .with_keys(&keys)?
        .build()?;

    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "Miditoroblox",
        options,
        Box::new(|cc| Ok(Box::new(MidiApp::new(cc, device)))),
    ).map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

    Ok(())
}
