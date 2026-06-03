#[cfg(target_os = "linux")]
pub use evdev::KeyCode;

#[cfg(target_os = "windows")]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct KeyCode(pub u16);

#[cfg(target_os = "windows")]
impl KeyCode {
    pub const KEY_RESERVED: KeyCode = KeyCode(0);
    pub const KEY_1: KeyCode = KeyCode(0x31);
    pub const KEY_2: KeyCode = KeyCode(0x32);
    pub const KEY_3: KeyCode = KeyCode(0x33);
    pub const KEY_4: KeyCode = KeyCode(0x34);
    pub const KEY_5: KeyCode = KeyCode(0x35);
    pub const KEY_6: KeyCode = KeyCode(0x36);
    pub const KEY_7: KeyCode = KeyCode(0x37);
    pub const KEY_8: KeyCode = KeyCode(0x38);
    pub const KEY_9: KeyCode = KeyCode(0x39);
    pub const KEY_0: KeyCode = KeyCode(0x30);
    pub const KEY_Q: KeyCode = KeyCode(0x51);
    pub const KEY_W: KeyCode = KeyCode(0x57);
    pub const KEY_E: KeyCode = KeyCode(0x45);
    pub const KEY_R: KeyCode = KeyCode(0x52);
    pub const KEY_T: KeyCode = KeyCode(0x54);
    pub const KEY_Y: KeyCode = KeyCode(0x59);
    pub const KEY_U: KeyCode = KeyCode(0x55);
    pub const KEY_I: KeyCode = KeyCode(0x49);
    pub const KEY_O: KeyCode = KeyCode(0x4F);
    pub const KEY_P: KeyCode = KeyCode(0x50);
    pub const KEY_A: KeyCode = KeyCode(0x41);
    pub const KEY_S: KeyCode = KeyCode(0x53);
    pub const KEY_D: KeyCode = KeyCode(0x44);
    pub const KEY_F: KeyCode = KeyCode(0x46);
    pub const KEY_G: KeyCode = KeyCode(0x47);
    pub const KEY_H: KeyCode = KeyCode(0x48);
    pub const KEY_J: KeyCode = KeyCode(0x4A);
    pub const KEY_K: KeyCode = KeyCode(0x4B);
    pub const KEY_L: KeyCode = KeyCode(0x4C);
    pub const KEY_Z: KeyCode = KeyCode(0x5A);
    pub const KEY_X: KeyCode = KeyCode(0x58);
    pub const KEY_C: KeyCode = KeyCode(0x43);
    pub const KEY_V: KeyCode = KeyCode(0x56);
    pub const KEY_B: KeyCode = KeyCode(0x42);
    pub const KEY_N: KeyCode = KeyCode(0x4E);
    pub const KEY_M: KeyCode = KeyCode(0x4D);

    pub const KEY_LEFTSHIFT: KeyCode = KeyCode(0xA0); // VK_LSHIFT
    pub const KEY_LEFTCTRL: KeyCode = KeyCode(0xA2);  // VK_LCONTROL
    pub const KEY_LEFTALT: KeyCode = KeyCode(0xA4);   // VK_LMENU
    pub const KEY_RIGHTALT: KeyCode = KeyCode(0xA5);  // VK_RMENU
    pub const KEY_UP: KeyCode = KeyCode(0x26);        // VK_UP
    pub const KEY_DOWN: KeyCode = KeyCode(0x28);      // VK_DOWN
    pub const KEY_APOSTROPHE: KeyCode = KeyCode(0xDE); // VK_OEM_7

    pub fn code(&self) -> u16 {
        self.0
    }
}

use serde::Deserialize;
use std::collections::HashMap;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SolverMode {
    Efficiency,
    Accuracy, // Ignores the max jump
}

#[derive(Clone, Copy, Debug)]
pub struct KeyMapping {
    pub midi_note: u8,
    pub key_code: KeyCode,
    pub shift: bool,
    pub ctrl: bool,
}

#[derive(Deserialize)]
struct JsonKeyMapping {
    midi_note: u8,
    key: String,
    shift: bool,
    ctrl: bool,
}

fn parse_key_str(k: &str) -> KeyCode {
    match k {
        "KEY_1" => KeyCode::KEY_1,
        "KEY_2" => KeyCode::KEY_2,
        "KEY_3" => KeyCode::KEY_3,
        "KEY_4" => KeyCode::KEY_4,
        "KEY_5" => KeyCode::KEY_5,
        "KEY_6" => KeyCode::KEY_6,
        "KEY_7" => KeyCode::KEY_7,
        "KEY_8" => KeyCode::KEY_8,
        "KEY_9" => KeyCode::KEY_9,
        "KEY_0" => KeyCode::KEY_0,
        "KEY_Q" => KeyCode::KEY_Q,
        "KEY_W" => KeyCode::KEY_W,
        "KEY_E" => KeyCode::KEY_E,
        "KEY_R" => KeyCode::KEY_R,
        "KEY_T" => KeyCode::KEY_T,
        "KEY_Y" => KeyCode::KEY_Y,
        "KEY_U" => KeyCode::KEY_U,
        "KEY_I" => KeyCode::KEY_I,
        "KEY_O" => KeyCode::KEY_O,
        "KEY_P" => KeyCode::KEY_P,
        "KEY_A" => KeyCode::KEY_A,
        "KEY_S" => KeyCode::KEY_S,
        "KEY_D" => KeyCode::KEY_D,
        "KEY_F" => KeyCode::KEY_F,
        "KEY_G" => KeyCode::KEY_G,
        "KEY_H" => KeyCode::KEY_H,
        "KEY_J" => KeyCode::KEY_J,
        "KEY_K" => KeyCode::KEY_K,
        "KEY_L" => KeyCode::KEY_L,
        "KEY_Z" => KeyCode::KEY_Z,
        "KEY_X" => KeyCode::KEY_X,
        "KEY_C" => KeyCode::KEY_C,
        "KEY_V" => KeyCode::KEY_V,
        "KEY_B" => KeyCode::KEY_B,
        "KEY_N" => KeyCode::KEY_N,
        "KEY_M" => KeyCode::KEY_M,
        _ => KeyCode::KEY_RESERVED,
    }
}

pub fn get_available_mappings() -> Vec<KeyMapping> {
    let json_data = include_str!("../mappings.json");
    let json_mappings: Vec<JsonKeyMapping> =
        serde_json::from_str(json_data).expect("Failed to parse mappings.json");

    json_mappings
        .into_iter()
        .map(|m| KeyMapping {
            midi_note: m.midi_note,
            key_code: parse_key_str(&m.key),
            shift: m.shift,
            ctrl: m.ctrl,
        })
        .collect()
}

pub struct Solver {
    // Tracks which physical keys are currently occupied by which MIDI note
    // KeyCode -> List of Active Midi Notes (implied, though really we only care if it's pressed)
    // Holding a key holds the note.
    pub active_keys: HashMap<KeyCode, (u8, Instant)>,

    pub shift_active: bool,
    pub ctrl_active: bool,

    // The current global transposition offset
    pub current_transpose: i32,
}

impl Solver {
    pub fn new() -> Self {
        Self {
            active_keys: HashMap::new(),
            shift_active: false,
            ctrl_active: false,
            current_transpose: 0,
        }
    }

    pub fn solve(
        &self,
        target_note: u8,
        mode: SolverMode,
        max_jump: i32,
        transpose_range: i32,
    ) -> Option<(i32, KeyMapping)> {
        self.solve_bounded(target_note, mode, max_jump, -transpose_range, transpose_range)
    }

    pub fn solve_bounded(
        &self,
        target_note: u8,
        mode: SolverMode,
        max_jump: i32,
        min_transpose: i32,
        max_transpose: i32,
    ) -> Option<(i32, KeyMapping)> {
        let mappings = get_available_mappings();

        let mut best_candidate: Option<(i32, KeyMapping)> = None;
        let mut best_score = i32::MAX;

        for map in &mappings {
            let required_transpose = target_note as i32 - map.midi_note as i32;

            if required_transpose < min_transpose || required_transpose > max_transpose {
                continue;
            }

            let distance = (required_transpose - self.current_transpose).abs();

            if mode == SolverMode::Efficiency && distance > max_jump {
                continue;
            }

            let mut score = distance * 1000;

            if let Some((held_note, start_time)) = self.active_keys.get(&map.key_code) {
                // It's a busy key.
                if *held_note == target_note {
                    // Retriggering the same physical note is perfectly normal.
                    score += 100;
                } else {
                    // Stealing the key from a different note (polyphony theft).
                    // The newer the note, the more massively penalized it is.
                    let age_ms = start_time.elapsed().as_millis() as i32;
                    let mut theft_penalty = 10000 - (age_ms * 10);
                    if theft_penalty < 500 {
                        theft_penalty = 500;
                    }

                    score += theft_penalty;
                }
            }

            // Small penalty for shift/ctrl modifications.
            if self.shift_active != map.shift {
                score += 5;
            }
            if self.ctrl_active != map.ctrl {
                score += 5;
            }

            // Prefer mappings closer to the center of the keyboard (midi 60)
            let center_dist = (map.midi_note as i32 - 60).abs();
            score += center_dist;

            if score < best_score {
                best_score = score;
                best_candidate = Some((required_transpose, *map));
            }
        }

        best_candidate
    }

    pub fn register_note_on(
        &mut self,
        key: KeyCode,
        note: u8,
        transpose: i32,
        shift: bool,
        ctrl: bool,
    ) {
        self.active_keys.insert(key, (note, Instant::now()));
        self.current_transpose = transpose;
        self.shift_active = shift;
        self.ctrl_active = ctrl;
    }

    pub fn register_note_off(&mut self, note: u8) -> Option<KeyCode> {
        // Find the physical key mapped to this MIDI note.
        let mut key_to_release = None;

        for (code, (held_note, _)) in self.active_keys.iter() {
            if *held_note == note {
                key_to_release = Some(*code);
                break;
            }
        }

        if let Some(code) = key_to_release {
            self.active_keys.remove(&code);
        }

        // If no keys left, modifiers are free (conceptually)
        if self.active_keys.is_empty() {
            self.shift_active = false;
            self.ctrl_active = false;
        }

        key_to_release
    }

    pub fn reset_keys(&mut self) -> Vec<KeyCode> {
        let keys: Vec<KeyCode> = self.active_keys.keys().cloned().collect();
        self.active_keys.clear();
        self.shift_active = false;
        self.ctrl_active = false;
        keys
    }

    pub fn reset_transpose(&mut self) {
        self.current_transpose = 0;
    }
}
