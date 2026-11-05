#[cfg(target_os = "linux")]
pub use evdev::KeyCode;

#[cfg(target_os = "windows")]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct KeyCode(pub u16);

#[cfg(target_os = "windows")]
impl KeyCode {
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
    pub const KEY_LEFTCTRL: KeyCode = KeyCode(0xA2); // VK_LCONTROL
    pub const KEY_LEFTALT: KeyCode = KeyCode(0xA4); // VK_LMENU
    pub const KEY_RIGHTALT: KeyCode = KeyCode(0xA5); // VK_RMENU
    pub const KEY_UP: KeyCode = KeyCode(0x26); // VK_UP
    pub const KEY_DOWN: KeyCode = KeyCode(0x28); // VK_DOWN
    pub const KEY_APOSTROPHE: KeyCode = KeyCode(0xDE); // VK_OEM_7

    pub fn code(&self) -> u16 {
        self.0
    }
}

use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Instant;

pub fn jump_budget(accuracy: u64) -> i32 {
    3 + (45 * accuracy.min(100) / 100) as i32
}

pub struct SolveRequest {
    pub target: u8,
    pub max_jump: i32,
    pub transpose_bounds: (i32, i32),
    pub physical_range: (u8, u8),
    pub groups: MappingGroups,
    pub now: Instant,
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

fn parse_key_str(k: &str) -> Option<KeyCode> {
    Some(match k {
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
        _ => return None,
    })
}

fn parse_mappings(json: &str) -> Result<Vec<KeyMapping>, String> {
    let entries: Vec<JsonKeyMapping> = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let mut notes = std::collections::HashSet::new();
    entries
        .into_iter()
        .map(|m| {
            if !(21..=108).contains(&m.midi_note) || !notes.insert(m.midi_note) {
                return Err(format!("Invalid or duplicate MIDI note {}", m.midi_note));
            }
            Ok(KeyMapping {
                midi_note: m.midi_note,
                key_code: parse_key_str(&m.key)
                    .ok_or_else(|| format!("Unknown mapping key: {}", m.key))?,
                shift: m.shift,
                ctrl: m.ctrl,
            })
        })
        .collect()
}

pub fn get_available_mappings() -> &'static [KeyMapping] {
    static MAPPINGS: OnceLock<Vec<KeyMapping>> = OnceLock::new();
    MAPPINGS
        .get_or_init(|| {
            parse_mappings(include_str!("../mappings.json"))
                .expect("Invalid embedded mappings.json")
        })
        .as_slice()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MappingGroups {
    pub low: bool,
    pub middle: bool,
    pub high: bool,
}

impl MappingGroups {
    pub fn contains(self, note: u8) -> bool {
        match note {
            21..=35 => self.low,
            36..=96 => self.middle,
            97..=108 => self.high,
            _ => false,
        }
    }
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

    pub fn solve_bounded(&self, request: SolveRequest) -> Option<(i32, KeyMapping)> {
        let mut best = None;
        let mut best_rank = None;
        for (index, map) in get_available_mappings().iter().enumerate() {
            if !request.groups.contains(map.midi_note)
                || !(request.physical_range.0..=request.physical_range.1).contains(&map.midi_note)
            {
                continue;
            }
            let transpose = request.target as i32 - map.midi_note as i32;
            let distance = (transpose - self.current_transpose).abs();
            if !(request.transpose_bounds.0..=request.transpose_bounds.1).contains(&transpose)
                || distance > request.max_jump
            {
                continue;
            }
            let stolen_age = self
                .active_keys
                .get(&map.key_code)
                .filter(|(note, _)| *note != request.target)
                .map(|(_, start)| request.now.saturating_duration_since(*start));
            // Free/shared keys first; when theft is necessary, oldest voice first.
            let rank = (
                stolen_age.is_some(),
                std::cmp::Reverse(stolen_age.unwrap_or_default()),
                distance,
                transpose.abs(),
                u8::from(self.shift_active != map.shift) + u8::from(self.ctrl_active != map.ctrl),
                index,
            );
            if best_rank.is_none_or(|previous| rank < previous) {
                best_rank = Some(rank);
                best = Some((transpose, *map));
            }
        }
        best
    }

    pub fn register_note_on(
        &mut self,
        key: KeyCode,
        note: u8,
        transpose: i32,
        shift: bool,
        ctrl: bool,
        now: Instant,
    ) {
        self.active_keys.insert(key, (note, now));
        self.current_transpose = transpose;
        self.shift_active = shift;
        self.ctrl_active = ctrl;
    }

    pub fn reset_keys(&mut self) {
        self.active_keys.clear();
        self.shift_active = false;
        self.ctrl_active = false;
    }

    pub fn reset_transpose(&mut self) {
        self.current_transpose = 0;
    }
}

pub fn key_from_code(code: u16) -> KeyCode {
    #[cfg(target_os = "linux")]
    {
        KeyCode::new(code)
    }
    #[cfg(target_os = "windows")]
    {
        KeyCode(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(target: u8, now: Instant) -> SolveRequest {
        SolveRequest {
            target,
            max_jump: 48,
            transpose_bounds: (-24, 24),
            physical_range: (21, 108),
            groups: MappingGroups {
                low: true,
                middle: true,
                high: true,
            },
            now,
        }
    }

    #[test]
    fn slider_budget_endpoints_and_large_jump_skip() {
        assert_eq!(jump_budget(0), 3);
        assert_eq!(jump_budget(50), 25);
        assert_eq!(jump_budget(100), 48);
        assert_eq!(jump_budget(u64::MAX), 48);
        let mut solver = Solver::new();
        solver.current_transpose = -24;
        let mut r = request(84, Instant::now());
        r.physical_range = (60, 60);
        r.max_jump = jump_budget(0);
        assert!(solver.solve_bounded(r).is_none());
        let mut r = request(84, Instant::now());
        r.physical_range = (60, 60);
        assert_eq!(solver.solve_bounded(r).unwrap().0, 24);
    }

    #[test]
    fn free_keys_outrank_shorter_moves_and_same_pitch_is_shareable() {
        let now = Instant::now();
        let mut solver = Solver::new();
        solver.active_keys.insert(KeyCode::KEY_T, (61, now));
        let (transpose, map) = solver.solve_bounded(request(60, now)).unwrap();
        assert_ne!(map.key_code, KeyCode::KEY_T);
        assert_ne!(transpose, 0);
        solver.active_keys.insert(KeyCode::KEY_T, (60, now));
        let (transpose, map) = solver.solve_bounded(request(60, now)).unwrap();
        assert_eq!(transpose, 0);
        assert_eq!(map.key_code, KeyCode::KEY_T);
    }

    #[test]
    fn unavoidable_theft_prefers_oldest_even_over_shorter_move() {
        let now = Instant::now();
        let mut solver = Solver::new();
        solver.active_keys.insert(KeyCode::KEY_T, (1, now));
        solver
            .active_keys
            .insert(KeyCode::KEY_Y, (2, now - std::time::Duration::from_secs(2)));
        let mut r = request(60, now);
        r.physical_range = (60, 62);
        let (transpose, map) = solver.solve_bounded(r).unwrap();
        assert_eq!(transpose, -2);
        assert_eq!(map.key_code, KeyCode::KEY_Y);
    }

    #[test]
    fn candidates_obey_ranges_groups_bounds_and_stable_ties() {
        let now = Instant::now();
        let mut solver = Solver::new();
        solver.current_transpose = 1;
        // With the current physical key busy, equally close alternatives favor zero.
        solver.active_keys.insert(KeyCode::KEY_3, (1, now));
        let (transpose, _) = solver.solve_bounded(request(41, now)).unwrap();
        assert_eq!(transpose, 0);
        for target in 0..=127 {
            for current in [-24, -5, 0, 5, 24] {
                solver.current_transpose = current;
                let mut r = request(target, now);
                r.physical_range = (48, 72);
                r.transpose_bounds = (-12, 12);
                r.max_jump = 3;
                if let Some((transpose, map)) = solver.solve_bounded(r) {
                    assert!((48..=72).contains(&map.midi_note));
                    assert!((-12..=12).contains(&transpose));
                    assert!((transpose - current).abs() <= 3);
                    assert_eq!(map.midi_note as i32 + transpose, target as i32);
                }
            }
        }
        let mut r = request(60, now);
        r.groups.middle = false;
        r.physical_range = (48, 72);
        assert!(solver.solve_bounded(r).is_none());
        solver.current_transpose = 0;
        let first = solver.solve_bounded(request(60, now)).unwrap();
        for _ in 0..10 {
            let next = solver.solve_bounded(request(60, now)).unwrap();
            assert_eq!((first.0, first.1.key_code), (next.0, next.1.key_code));
        }
    }

    #[test]
    fn embedded_mappings_are_valid_and_cached() {
        assert_eq!(get_available_mappings().len(), 88);
        assert!(std::ptr::eq(
            get_available_mappings(),
            get_available_mappings()
        ));
    }
    #[test]
    fn invalid_mapping_keys_and_duplicate_notes_are_rejected() {
        assert!(
            parse_mappings(r#"[{"midi_note":60,"key":"TYPO","shift":false,"ctrl":false}]"#)
                .is_err()
        );
        let entry = r#"{"midi_note":60,"key":"KEY_T","shift":false,"ctrl":false}"#;
        assert!(parse_mappings(&format!("[{entry},{entry}]")).is_err());
    }
}
