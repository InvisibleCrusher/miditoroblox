use evdev::KeyCode;
use std::collections::{HashMap, HashSet};
use serde::Deserialize;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SolverMode {
    Efficiency, // Least clicks
    Accuracy,   // Best accuracy
}

#[derive(Clone, Copy, Debug)]
pub struct KeyMapping {
    pub midi_note: u8,
    pub key_code: KeyCode,
    pub shift: bool,
    pub ctrl: bool,
}

// Standard key mappings

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
    let json_mappings: Vec<JsonKeyMapping> = serde_json::from_str(json_data)
        .expect("Failed to parse mappings.json");

    json_mappings.into_iter().map(|m| KeyMapping {
        midi_note: m.midi_note,
        key_code: parse_key_str(&m.key),
        shift: m.shift,
        ctrl: m.ctrl,
    }).collect()
}

pub struct Solver {
    // Tracks which physical keys are currently occupied by which MIDI note
    // KeyCode -> List of Active Midi Notes (implied, though really we only care if it's pressed)
    // Holding a key holds the note.
    pub active_keys: HashMap<KeyCode, HashSet<u8>>, 
    
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

    /// Try to find a solution to play `target_note`.
    /// Returns: Option<(new_transpose_offset, key_mapping_to_use)>
    pub fn solve(
        &self,
        target_note: u8,
        mode: SolverMode,
        max_jump: i32,
        transpose_range: i32 // 24 means -24 to +24
    ) -> Option<(i32, KeyMapping)> {
        let mappings = get_available_mappings();

        // Potential solution candidates
        let mut best_candidate: Option<(i32, KeyMapping)> = None;
        let mut min_distance = i32::MAX;

        // Find required transposition T = target_note - map.midi_note
        for map in &mappings {
            let required_transpose = target_note as i32 - map.midi_note as i32;
            
            // Check if required transpose is within global range limits
            if required_transpose.abs() > transpose_range {
                continue;
            }

            // Check if this physical key is currently pressed
            let key_busy = self.active_keys.contains_key(&map.key_code) && !self.active_keys[&map.key_code].is_empty();
            
            // Check modifiers conflict
            if !self.is_modifier_safe(map) {
                continue;
            }

            let mut distance = (required_transpose - self.current_transpose).abs();
            
            // Penalty for stealing a busy key (we prefer free keys via transposition)
            if key_busy {
                distance += 100; // Equivalent to 100 semitones jump, so we only do it if necessary
            }

            match mode {
                SolverMode::Efficiency => {
                    // Must be within max_jump
                    if distance <= max_jump {
                        if distance < min_distance {
                            min_distance = distance;
                            best_candidate = Some((required_transpose, *map));
                        }
                    }
                },
                SolverMode::Accuracy => {
                    // Just find any valid one. Preference for closer distance?
                    if distance < min_distance {
                        min_distance = distance;
                        best_candidate = Some((required_transpose, *map));
                    }
                }
            }
        }

        best_candidate
    }

    // Check if activating modifiers for 'new_map' would disrupt currently held notes
    fn is_modifier_safe(&self, new_map: &KeyMapping) -> bool {
        // Iterate over all active keys
        for (_code, notes) in &self.active_keys {
            if notes.is_empty() { continue; }
            
            // Ensure modifier compatibility.
            // All active keys must share the same Shift/Ctrl requirement as the new candidate
            // to avoid disrupting currently held notes.

            
            // We need to know the 'modifier state' of the active keys.
            // Since we track `shift_active` and `ctrl_active`, we can check against that.
            
            if self.shift_active != new_map.shift {
                return false;
            }
            if self.ctrl_active != new_map.ctrl {
                return false;
            }
        }
        true
    }

    pub fn register_note_on(&mut self, key: KeyCode, note: u8, transpose: i32, shift: bool, ctrl: bool) {
        self.active_keys.entry(key).or_insert_with(HashSet::new).insert(note);
        self.current_transpose = transpose;
        self.shift_active = shift;
        self.ctrl_active = ctrl;
    }

    pub fn register_note_off(&mut self, note: u8) -> Option<KeyCode> {
        // Find the physical key mapped to this MIDI note.
        let mut key_to_release = None;
        
        for (code, notes) in self.active_keys.iter_mut() {
            if notes.contains(&note) {
                notes.remove(&note);
                if notes.is_empty() {
                    key_to_release = Some(*code);
                }
                break;
            }
        }
        
        // If no keys left, modifiers are free (conceptually), but we update them lazily only on new press
        // or we could track if count==0.
        
        if self.active_keys.values().all(|s| s.is_empty()) {
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
