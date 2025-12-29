use evdev::KeyCode;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SolverMode {
    Efficiency, // Least clicks (limited jump)
    Accuracy,   // Best accuracy (find any solution)
}

#[derive(Clone, Copy, Debug)]
pub struct KeyMapping {
    pub midi_note: u8,
    pub key_code: KeyCode,
    pub shift: bool,
    pub ctrl: bool,
}

// We'll move the huge mapping list here or access it. 
// For now, let's include the standard mappings here to avoid circular dep issues if we tried to share from main initially.
// We can refactor main to use this later.
pub fn get_available_mappings() -> Vec<KeyMapping> {
    vec![
        // --- Low Range (A0 to B1) - [CTRL] ---
        KeyMapping { midi_note: 21, key_code: KeyCode::KEY_1, shift: false, ctrl: true }, // A0  -> 1
        KeyMapping { midi_note: 22, key_code: KeyCode::KEY_2, shift: false, ctrl: true }, // A#0 -> 2
        KeyMapping { midi_note: 23, key_code: KeyCode::KEY_3, shift: false, ctrl: true }, // B0  -> 3
        KeyMapping { midi_note: 24, key_code: KeyCode::KEY_4, shift: false, ctrl: true }, // C1  -> 4
        KeyMapping { midi_note: 25, key_code: KeyCode::KEY_5, shift: false, ctrl: true }, // C#1 -> 5
        KeyMapping { midi_note: 26, key_code: KeyCode::KEY_6, shift: false, ctrl: true }, // D1  -> 6
        KeyMapping { midi_note: 27, key_code: KeyCode::KEY_7, shift: false, ctrl: true }, // D#1 -> 7
        KeyMapping { midi_note: 28, key_code: KeyCode::KEY_8, shift: false, ctrl: true }, // E1  -> 8
        KeyMapping { midi_note: 29, key_code: KeyCode::KEY_9, shift: false, ctrl: true }, // F1  -> 9
        KeyMapping { midi_note: 30, key_code: KeyCode::KEY_0, shift: false, ctrl: true }, // F#1 -> 0
        KeyMapping { midi_note: 31, key_code: KeyCode::KEY_Q, shift: false, ctrl: true }, // G1  -> q
        KeyMapping { midi_note: 32, key_code: KeyCode::KEY_W, shift: false, ctrl: true }, // G#1 -> w
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

pub struct Solver {
    // Tracks which physical keys are currently occupied by which MIDI note (post-transpose solution)
    // KeyCode -> List of Active Midi Notes (implied, though really we only care if it's pressed)
    // But since one physical key could theoretically "simulate" multiple notes if we were super smart (unlikely),
    // let's just track simply: KeyCode -> Count of users.
    // Actually, physically pressing a key twice is a no-op or a re-trigger depending on app.
    // Roblox piano usually: pressing again stops it? Or re-attacks?
    // User says: "the C4 will only stop playing if we stop pressing (or press again) the B3 key"
    // So holding it holds the note.
    pub active_keys: HashMap<KeyCode, HashSet<u8>>, 
    
    // We also need to track global modifiers since they are shared
    pub shift_active: bool,
    pub ctrl_active: bool,
    
    // What is the current global transposition offset? (0 means C4=C4)
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
        transpose_range: i32 // e.g. 24 means -24 to +24
    ) -> Option<(i32, KeyMapping)> {
        let mappings = get_available_mappings();

        // Potential solution candidates
        let mut best_candidate: Option<(i32, KeyMapping)> = None;
        let mut min_distance = i32::MAX;

        // Iterate through all possible transpose offsets to see if we can produce the target note with an available key
        // A "virtual note" played at `key_note` with offset `T` produces `real_note = key_note + T`
        // We want `real_note == target_note` => `key_note = target_note - T`
        // So we need to look for a physical mapping for `target_note - T`
        
        // Search range for T: [-transpose_range, +transpose_range]
        // But we can optimize: we iterate over available mappings, and see what T is required.
        // T = target_note - map.midi_note (as i32)
        
        for map in &mappings {
            let required_transpose = target_note as i32 - map.midi_note as i32;
            
            // Check if required transpose is within global range limits
            if required_transpose.abs() > transpose_range {
                continue;
            }

            // Check if this physical key is free to use?
            // "The solver, would for example look, and see that B3 key isn't pressed, so we transpose up by 1"
            // So we must check if KeyCode is currently pressed.
            if self.active_keys.contains_key(&map.key_code) && !self.active_keys[&map.key_code].is_empty() {
                // Key is busy holding another note. 
                // Wait, if it's holding the SAME note (re-trigger), maybe it's okay?
                // But usually we assume conflict if busy.
                // Let's assume strict conflict for now.
                continue;
            }

            // Check modifiers conflict?
            // If we need SHIFT, but SHIFT is "active" doing something incompatible?
            // Actually, in this generic solver, if we say "Press Shift+A", we assume we can press Shift.
            // But if another key is currently held that DOES NOT use shift, and we press shift, does it break the other key?
            // User didn't specify modifier interference, but usually Shift affects all keys.
            // "since the program can't play some of 2 notes at the same time ... (or even 3 with lower octaves)"
            // implies conflict.
            // If I am holding 'a' (no shift), and I press Shift+'b', 'a' becomes Shift+'a'.
            // If Shift+'a' maps to something else, we screw up the held note!
            // This is complex.
            // Let's check for Modifier Safety.
            if !self.is_modifier_safe(map) {
                continue;
            }

            let distance = (required_transpose - self.current_transpose).abs();

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
            
            // Find the mapping that was used for this key?
            // We don't store which mapping was used, just the key.
            // But we know the key must have been compatible with current modifiers OR the modifiers changed?
            // Wait, if we change Shift state, ALL currently held keys change meaning.
            // So we must ensure that for all held keys, their meaning doesn't change destructively?
            // Or simpler: We only allow Shift/Ctrl change if NO keys are held?
            // That's too restrictive.
            
            // Better heuristic:
            // If `new_map` requires Shift=True, and we have a held key that requires Shift=False:
            //   - Pressing Shift will turn the held key into its Shift variant.
            //   - Does that matter? Yes, it changes the note being sent or stops the original.
            //   - So we assume: All held keys must agree on Modifier State?
            //   - Typical keyboard matrix limitations or logic limitations.
            
            // If I am holding 'q' (G1), and I press Shift+'w' (G#4)...
            // The system sees Shift+q (unknown/different) and Shift+w.
            // So 'q' effectively releases or changes.
            
            // So: all active keys must share the same Shift/Ctrl requirement as the new candidate?
            // That basically means we can only play chords where all usage of Shift/Ctrl matches.
            // This is the "Modifier Conflict".
            // The Solver is supposed to solve "same key" conflict, but maybe it can solve Modifier conflict too?
            // By finding a mapping for the new note that MATCHES the current modifier state!
            
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
        // Find which key was effectively simulating this note?
        // This is tricky because `note` (input midi) + `transpose` (at time of press) = `mapped_note`.
        // Wait, NO. 
        // We solved: `target_note` = `physical_map_note` + `transpose`.
        // So `physical_map_note` = `target_note` - `transpose`.
        // But `transpose` might have changed since then?
        // User said: "The C4 will only stop playing if we stop pressing ... after it got played it doesn't care about transposition."
        // So we just need to find the physical key that we decided to use for this logical note.
        
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
