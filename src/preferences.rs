//! Serializable user preferences, excluding transport and output state.
use crate::{instrument::InstrumentSlot, midi_player::ScanDir, runtime::SharedState};
use std::sync::atomic::Ordering;
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Preferences {
    pub base_mapping_enabled: bool,
    pub low_mapping_enabled: bool,
    pub high_mapping_enabled: bool,
    pub auto_transpose_enabled: bool,
    pub experimental_transpose_enabled: bool,
    pub experimental_hold_ctrl_enabled: bool,
    pub quantize_enabled: bool,
    pub solver_enabled: bool,
    pub visualizer_enabled: bool,
    pub visualizer_show_midi: bool,
    pub visualizer_show_roblox: bool,
    pub panic_enabled: bool,
    pub control_velocity_enabled: bool,
    pub sustain_enabled: bool,
    pub use_midi_device: bool,
    pub range_instrument_enabled: bool,
    pub fold_octaves: bool,
    pub transpose_delay_ms: u64,
    pub quantize_ms: u64,
    pub solver_accuracy: u64,
    pub transpose_range: u64,
    pub velocity_delay_ms: u64,
    pub slots: [InstrumentSlot; 4],
    pub opacity: f32,
    pub always_on_top: bool,
    pub scan_dirs: Vec<ScanDir>,
    pub preferred_port: Option<String>,
}
impl Preferences {
    pub fn capture(
        shared: &SharedState,
        opacity: f32,
        always_on_top: bool,
        scan_dirs: Vec<ScanDir>,
        preferred_port: Option<String>,
    ) -> Self {
        Self {
            base_mapping_enabled: shared.base_mapping_enabled.load(Ordering::Relaxed),
            low_mapping_enabled: shared.low_mapping_enabled.load(Ordering::Relaxed),
            high_mapping_enabled: shared.high_mapping_enabled.load(Ordering::Relaxed),
            auto_transpose_enabled: shared.auto_transpose_enabled.load(Ordering::Relaxed),
            experimental_transpose_enabled: shared
                .experimental_transpose_enabled
                .load(Ordering::Relaxed),
            experimental_hold_ctrl_enabled: shared
                .experimental_hold_ctrl_enabled
                .load(Ordering::Relaxed),
            quantize_enabled: shared.quantize_enabled.load(Ordering::Relaxed),
            solver_enabled: shared.solver_enabled.load(Ordering::Relaxed),
            visualizer_enabled: shared.visualizer_enabled.load(Ordering::Relaxed),
            visualizer_show_midi: shared.visualizer_show_midi.load(Ordering::Relaxed),
            visualizer_show_roblox: shared.visualizer_show_roblox.load(Ordering::Relaxed),
            panic_enabled: shared.panic_enabled.load(Ordering::Relaxed),
            control_velocity_enabled: shared.control_velocity_enabled.load(Ordering::Relaxed),
            sustain_enabled: shared.sustain_enabled.load(Ordering::Relaxed),
            use_midi_device: shared.use_midi_device.load(Ordering::Relaxed),
            range_instrument_enabled: shared.range_instrument_enabled.load(Ordering::Relaxed),
            fold_octaves: shared.fold_octaves.load(Ordering::Relaxed),
            transpose_delay_ms: shared.transpose_delay_ms.load(Ordering::Relaxed),
            quantize_ms: shared.quantize_ms.load(Ordering::Relaxed),
            solver_accuracy: shared.solver_accuracy.load(Ordering::Relaxed),
            transpose_range: shared.transpose_range.load(Ordering::Relaxed),
            velocity_delay_ms: shared.velocity_delay_ms.load(Ordering::Relaxed),
            slots: *shared
                .instrument_slots
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
            opacity,
            always_on_top,
            scan_dirs,
            preferred_port,
        }
    }
    pub fn validate(&mut self) {
        self.opacity = if self.opacity.is_finite() {
            self.opacity.clamp(0.1, 1.0)
        } else {
            1.0
        };
        self.transpose_delay_ms = self.transpose_delay_ms.clamp(0, 1000);
        self.quantize_ms = self.quantize_ms.clamp(10, 500);
        self.solver_accuracy = self.solver_accuracy.clamp(0, 100);
        self.transpose_range = self.transpose_range.clamp(12, 24);
        self.velocity_delay_ms = self.velocity_delay_ms.clamp(0, 100);
        for slot in &mut self.slots {
            slot.max_note = slot.max_note.clamp(1, 88);
            slot.min_note = slot.min_note.clamp(1, slot.max_note);
            slot.transposition = slot.transposition.clamp(-24, 24);
        }
    }
    pub fn apply(&self, shared: &SharedState) {
        shared
            .base_mapping_enabled
            .store(self.base_mapping_enabled, Ordering::Relaxed);
        shared
            .low_mapping_enabled
            .store(self.low_mapping_enabled, Ordering::Relaxed);
        shared
            .high_mapping_enabled
            .store(self.high_mapping_enabled, Ordering::Relaxed);
        shared
            .auto_transpose_enabled
            .store(self.auto_transpose_enabled, Ordering::Relaxed);
        shared
            .experimental_transpose_enabled
            .store(self.experimental_transpose_enabled, Ordering::Relaxed);
        shared
            .experimental_hold_ctrl_enabled
            .store(self.experimental_hold_ctrl_enabled, Ordering::Relaxed);
        shared
            .quantize_enabled
            .store(self.quantize_enabled, Ordering::Relaxed);
        shared
            .solver_enabled
            .store(self.solver_enabled, Ordering::Relaxed);
        shared
            .visualizer_enabled
            .store(self.visualizer_enabled, Ordering::Relaxed);
        shared
            .visualizer_show_midi
            .store(self.visualizer_show_midi, Ordering::Relaxed);
        shared
            .visualizer_show_roblox
            .store(self.visualizer_show_roblox, Ordering::Relaxed);
        shared
            .panic_enabled
            .store(self.panic_enabled, Ordering::Relaxed);
        shared
            .control_velocity_enabled
            .store(self.control_velocity_enabled, Ordering::Relaxed);
        shared
            .sustain_enabled
            .store(self.sustain_enabled, Ordering::Relaxed);
        shared
            .use_midi_device
            .store(self.use_midi_device, Ordering::Relaxed);
        shared
            .range_instrument_enabled
            .store(self.range_instrument_enabled, Ordering::Relaxed);
        shared
            .fold_octaves
            .store(self.fold_octaves, Ordering::Relaxed);
        shared
            .transpose_delay_ms
            .store(self.transpose_delay_ms, Ordering::Relaxed);
        shared
            .quantize_ms
            .store(self.quantize_ms, Ordering::Relaxed);
        shared
            .solver_accuracy
            .store(self.solver_accuracy, Ordering::Relaxed);
        shared
            .transpose_range
            .store(self.transpose_range, Ordering::Relaxed);
        shared
            .velocity_delay_ms
            .store(self.velocity_delay_ms, Ordering::Relaxed);
        *shared
            .instrument_slots
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = self.slots;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preferences_roundtrip_all_fields_and_clamp_to_ui_limits() {
        use crate::instrument::InstrumentType;
        let mut prefs = Preferences {
            base_mapping_enabled: true,
            low_mapping_enabled: true,
            high_mapping_enabled: true,
            auto_transpose_enabled: true,
            experimental_transpose_enabled: true,
            experimental_hold_ctrl_enabled: true,
            quantize_enabled: true,
            solver_enabled: true,
            visualizer_enabled: true,
            visualizer_show_midi: true,
            visualizer_show_roblox: true,
            panic_enabled: true,
            control_velocity_enabled: true,
            sustain_enabled: true,
            use_midi_device: true,
            range_instrument_enabled: true,
            fold_octaves: true,
            transpose_delay_ms: u64::MAX,
            quantize_ms: u64::MAX,
            solver_accuracy: u64::MAX,
            transpose_range: u64::MAX,
            velocity_delay_ms: u64::MAX,
            always_on_top: true,
            slots: [InstrumentSlot {
                enabled: true,
                inst_type: InstrumentType::Strings,
                min_note: 100,
                max_note: 0,
                transposition: 100,
            }; 4],
            opacity: 0.0,
            scan_dirs: vec![ScanDir {
                path: "music".into(),
                recursive: true,
            }],
            preferred_port: Some("Preferred keyboard".into()),
        };
        let serialized = serde_json::to_vec(&prefs).unwrap();
        assert_eq!(
            serde_json::from_slice::<Preferences>(&serialized).unwrap(),
            prefs
        );
        prefs.validate();
        assert_eq!(prefs.opacity, 0.1);
        assert_eq!(prefs.transpose_delay_ms, 1000);
        assert_eq!(prefs.quantize_ms, 500);
        assert_eq!(prefs.solver_accuracy, 100);
        assert_eq!(prefs.transpose_range, 24);
        assert_eq!(prefs.velocity_delay_ms, 100);
        assert_eq!(
            (
                prefs.slots[0].min_note,
                prefs.slots[0].max_note,
                prefs.slots[0].transposition
            ),
            (1, 1, 24)
        );
        prefs.quantize_ms = 0;
        prefs.transpose_range = 0;
        prefs.opacity = f32::NAN;
        prefs.validate();
        assert_eq!(prefs.quantize_ms, 10);
        assert_eq!(prefs.transpose_range, 12);
        assert_eq!(prefs.opacity, 1.0);
        let json = serde_json::to_value(&prefs).unwrap();
        for runtime_field in [
            "transpose",
            "position",
            "panic_active",
            "active_notes",
            "detection",
        ] {
            assert!(json.get(runtime_field).is_none());
        }
    }
}
