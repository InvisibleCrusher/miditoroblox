mod autodetect;
mod instrument;
mod midi_event;
mod midi_player;
mod pipeline;
mod platform;
mod preferences;
mod runtime;
mod schedule;
mod settings;
mod solver;

use eframe::egui;
use instrument::{InstrumentSlot, InstrumentType};
use midi_event::{MidiEvent, MidiSource};
use midi_player::{MidiPlayer, PlaybackStatus};
use midir::{MidiInput, MidiInputConnection, MidiInputPort};
use pipeline::Pipeline;
use platform::VirtualDevice;
use runtime::{DeviceState, SharedState, invalidate_output, send_live_message};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
struct MidiApp {
    last_preferences: Option<preferences::Preferences>,
    persistence_error: Option<String>,
    midi_input: Option<MidiInput>,
    available_ports: Vec<(String, MidiInputPort)>,
    selected_port_name: Option<String>,
    connection: Option<MidiInputConnection<Arc<SharedState>>>,
    shared_state: Arc<SharedState>,
    status_message: String,
    window_opacity: f32,
    always_on_top: bool,
    midi_player: MidiPlayer,
    new_dir_input: String,
    new_dir_recursive: bool,
    player_seek_pos: Option<f64>,
    player_was_panic: bool,
    player_paused_by_focus: bool,
    workers: Vec<std::thread::JoinHandle<()>>,
    detection: Option<autodetect::Job>,
    detection_report: Option<(autodetect::Snapshot, autodetect::DetectionResult)>,
}

impl MidiApp {
    fn new(cc: &eframe::CreationContext<'_>, virtual_device: VirtualDevice) -> Self {
        let (tx, rx) = std::sync::mpsc::sync_channel::<MidiEvent>(4096);
        let output_epoch = Arc::new(AtomicU64::new(0));
        let midi_player = MidiPlayer::new(tx.clone(), output_epoch.clone());
        let mut app = Self {
            last_preferences: None,
            persistence_error: None,
            midi_input: None,
            available_ports: Vec::new(),
            selected_port_name: None,
            connection: None,
            shared_state: Arc::new(SharedState {
                shutdown: AtomicBool::new(false),
                focus_ready_at: Mutex::new(Instant::now()),
                transpose: AtomicI32::new(0),
                error: Mutex::new(None),
                device_state: Mutex::new(DeviceState {
                    device: virtual_device,
                    pipeline: Pipeline::new(),
                }),
                base_mapping_enabled: AtomicBool::new(true),
                low_mapping_enabled: AtomicBool::new(true),
                high_mapping_enabled: AtomicBool::new(true),
                auto_transpose_enabled: AtomicBool::new(true),
                experimental_transpose_enabled: AtomicBool::new(true),
                experimental_hold_ctrl_enabled: AtomicBool::new(true),
                transpose_delay_ms: AtomicU64::new(0),
                quantize_enabled: AtomicBool::new(false),
                quantize_ms: AtomicU64::new(100),
                solver_enabled: AtomicBool::new(true),
                solver_accuracy: AtomicU64::new(0),
                transpose_range: AtomicU64::new(24),
                active_notes: Mutex::new(std::collections::HashSet::new()),
                active_output_notes: Mutex::new(std::collections::HashSet::new()),
                visualizer_enabled: AtomicBool::new(false),
                visualizer_show_midi: AtomicBool::new(true),
                visualizer_show_roblox: AtomicBool::new(true),
                ui_context: Mutex::new(None),
                panic_enabled: AtomicBool::new(true),
                panic_active: AtomicBool::new(false),
                panic_releases_in_flight: AtomicU64::new(0),
                control_velocity_enabled: AtomicBool::new(true),
                velocity_delay_ms: AtomicU64::new(0),
                sustain_enabled: AtomicBool::new(true),
                midi_sender: tx,
                output_epoch,
                use_midi_device: AtomicBool::new(false),
                app_focused: AtomicBool::new(cc.egui_ctx.input(|i| i.focused)),
                range_instrument_enabled: AtomicBool::new(false),
                fold_octaves: AtomicBool::new(false),
                instrument_slots: Mutex::new([
                    InstrumentSlot {
                        enabled: true,
                        inst_type: InstrumentType::Piano,
                        min_note: 1,
                        max_note: 88,
                        transposition: 0,
                    },
                    InstrumentSlot {
                        enabled: false,
                        inst_type: InstrumentType::Piano,
                        min_note: 1,
                        max_note: 88,
                        transposition: 0,
                    },
                    InstrumentSlot {
                        enabled: false,
                        inst_type: InstrumentType::Piano,
                        min_note: 1,
                        max_note: 88,
                        transposition: 0,
                    },
                    InstrumentSlot {
                        enabled: false,
                        inst_type: InstrumentType::Piano,
                        min_note: 1,
                        max_note: 88,
                        transposition: 0,
                    },
                ]),
            }),
            status_message: "Ready".to_string(),
            window_opacity: 1.0,
            always_on_top: false,
            midi_player,
            new_dir_input: String::new(),
            new_dir_recursive: false,
            player_seek_pos: None,
            player_was_panic: false,
            player_paused_by_focus: false,
            workers: Vec::new(),
            detection: None,
            detection_report: None,
        };

        let defaults = preferences::Preferences::capture(
            &app.shared_state,
            app.window_opacity,
            app.always_on_top,
            app.midi_player.scan_dirs.clone(),
            app.selected_port_name.clone(),
        );
        let loaded = app
            .midi_player
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .read("settings.json", defaults);
        match loaded {
            Ok(mut prefs) => {
                prefs.validate();
                prefs.apply(&app.shared_state);
                app.window_opacity = prefs.opacity;
                app.always_on_top = prefs.always_on_top;
                app.selected_port_name = prefs.preferred_port.clone();
                app.midi_player.scan_dirs = prefs.scan_dirs.clone();
                app.last_preferences = Some(prefs);
            }
            Err(e) => {
                app.persistence_error = Some(e.clone());
                app.status_message = e;
            }
        }
        cc.egui_ctx
            .send_viewport_cmd(egui::ViewportCommand::WindowLevel(if app.always_on_top {
                egui::WindowLevel::AlwaysOnTop
            } else {
                egui::WindowLevel::Normal
            }));

        app.workers
            .push(runtime::spawn_output(app.shared_state.clone(), rx));
        app.workers
            .push(platform::spawn_panic_monitor(app.shared_state.clone()));

        // Initialize visuals (opaque default)
        let mut visuals = egui::Visuals::dark();
        visuals.window_fill = egui::Color32::from_black_alpha((app.window_opacity * 255.0) as u8);
        visuals.panel_fill = egui::Color32::from_black_alpha((app.window_opacity * 255.0) as u8);
        cc.egui_ctx.set_visuals(visuals);

        app.refresh_ports();
        if !app.midi_player.scan_dirs.is_empty() {
            app.midi_player.scan();
        }
        app
    }

    fn save_preferences(&mut self, force: bool) {
        let prefs = preferences::Preferences::capture(
            &self.shared_state,
            self.window_opacity,
            self.always_on_top,
            self.midi_player.scan_dirs.clone(),
            self.selected_port_name.clone(),
        );
        let mut store = self
            .midi_player
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if self.last_preferences.as_ref() != Some(&prefs) {
            store.queue("settings.json", &prefs);
            self.last_preferences = Some(prefs);
        }
        for error in store.flush(force) {
            self.status_message = error.clone();
            self.persistence_error = Some(error);
        }
    }

    fn editor_ui(&mut self, ui: &mut egui::Ui) {
        let Some((path, document, mutes)) = &self.midi_player.editor else {
            if let Some(error) = &self.midi_player.editor_error {
                ui.colored_label(egui::Color32::YELLOW, error);
            } else if self.midi_player.selected_idx.is_some() {
                ui.label("Loading instrument editor…");
            }
            return;
        };
        if self
            .midi_player
            .selected_idx
            .and_then(|i| self.midi_player.midi_files.get(i))
            != Some(path)
        {
            return;
        }
        let document = document.clone();
        let mut edited = mutes.clone();
        ui.collapsing("Instrument editor", |ui| {
            ui.small("Mutes are saved for this MIDI. Playback and auto-detect use these edits.");
            ui.horizontal(|ui| {
                if ui.button("Mute all").clicked() {
                    edited.muted = document.instruments.iter().map(|row| row.id).collect();
                }
                if ui.button("Unmute all").clicked() {
                    edited.muted.clear();
                }
            });
            if document.instruments.is_empty() {
                ui.label("No instrument notes in this MIDI.");
            }
            for row in &document.instruments {
                let mut muted = edited.muted.contains(&row.id);
                let name = if row.id.percussion {
                    "Percussion"
                } else {
                    instrument::program_name(row.id.program)
                };
                let track = if row.track_name.is_empty() {
                    format!("Track {}", row.id.track + 1)
                } else {
                    format!("Track {}: {}", row.id.track + 1, row.track_name)
                };
                let channels = row
                    .channels
                    .iter()
                    .map(u8::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                ui.push_id(row.id, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        if ui.checkbox(&mut muted, "Mute").changed() {
                            if muted {
                                edited.muted.insert(row.id);
                            } else {
                                edited.muted.remove(&row.id);
                            }
                        }
                        ui.label(format!(
                            "{track} — {name}{} — channels {channels} — {} notes",
                            if row.id.percussion {
                                String::new()
                            } else {
                                format!(" (program {})", row.id.program + 1)
                            },
                            row.notes
                        ));
                    });
                });
            }
        });
        if &edited != mutes {
            self.midi_player.apply_mutes(edited);
        }
    }

    fn detection_snapshot(&self) -> Option<autodetect::Snapshot> {
        let path = self
            .midi_player
            .midi_files
            .get(self.midi_player.selected_idx?)
            .cloned()?;
        let (editor_path, document, mutes) = self.midi_player.editor.as_ref()?;
        if editor_path != &path {
            return None;
        }
        Some(autodetect::Snapshot {
            content_hash: Some(document.hash.clone()),
            mutes: mutes.clone(),
            file_stamp: autodetect::Snapshot::stamp(&path),
            path,
            config: runtime::config(&self.shared_state),
            quantize: self.shared_state.quantize_enabled.load(Ordering::Relaxed),
            quantize_ms: self.shared_state.quantize_ms.load(Ordering::Relaxed),
            accuracy: self.shared_state.solver_accuracy.load(Ordering::Relaxed),
            live: self.shared_state.use_midi_device.load(Ordering::Relaxed),
        })
    }

    fn poll_detection(&mut self) {
        if self.detection.is_none() && self.detection_report.is_none() {
            return;
        }
        let current = self.detection_snapshot();
        if self
            .detection_report
            .as_ref()
            .is_some_and(|(input, _)| Some(input) != current.as_ref())
        {
            self.detection_report = None;
        }
        let Some(job) = self.detection.as_mut() else {
            return;
        };
        if Some(&job.snapshot) != current.as_ref() {
            job.cancel.store(true, Ordering::Relaxed);
        }
        let Some(result) = job.poll() else {
            return;
        };
        let job = self.detection.take().unwrap();
        if !job.is_current(current.as_ref()) {
            self.status_message =
                "Auto-detect cancelled; settings kept (inputs changed or cancellation requested)"
                    .into();
            return;
        }
        match result {
            Ok(result) => {
                *self
                    .shared_state
                    .instrument_slots
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = result.slots;
                let mut input = job.snapshot.clone();
                input.config.slots = result.slots;
                self.detection_report = Some((input, result));
                self.status_message =
                    "Auto-detect applied. Copy the displayed ranges and transposes into Roblox."
                        .into();
            }
            Err(error) => self.status_message = format!("Auto-detect: {error}"),
        }
    }

    fn detection_ui(&mut self, ui: &mut egui::Ui) {
        let mut fold = self.shared_state.fold_octaves.load(Ordering::Relaxed);
        if ui.checkbox(&mut fold, "Fold out-of-range notes by octaves").on_hover_text("Move notes into their instrument range by octaves, preserving pitch class. Notes with no octave equivalent in the range are skipped.").changed() {
            self.shared_state.fold_octaves.store(fold, Ordering::Relaxed);
        }
        let snapshot = self.detection_snapshot();
        let validation = snapshot.as_ref().map_or_else(
            || Err("Select a MIDI file first".to_string()),
            |s| autodetect::validate(&s.config),
        );
        if let Some(job) = &self.detection {
            ui.horizontal(|ui| {
                ui.spinner();
                let phase = match job.phase.load(Ordering::Relaxed) {
                    0 => "Reading MIDI",
                    1 => "Finding range layouts",
                    2 => "Simulating layouts",
                    _ => "Refining layouts",
                };
                ui.label(format!(
                    "{phase} — {} simulations (up to 1,000)",
                    job.completed.load(Ordering::Relaxed)
                ));
                if ui
                    .add_enabled(
                        !job.cancel.load(Ordering::Relaxed),
                        egui::Button::new("Cancel auto-detect"),
                    )
                    .clicked()
                {
                    job.cancel.store(true, Ordering::Relaxed);
                }
            });
        } else {
            let button = ui.add_enabled(
                validation.is_ok(),
                egui::Button::new("Auto-detect settings"),
            );
            if button.clicked()
                && let Some(snapshot) = snapshot
            {
                self.detection_report = None;
                self.detection = Some(autodetect::Job::start(snapshot));
            }
            if let Err(reason) = validation {
                button.on_hover_text(reason.clone());
                ui.label(reason);
            }
        }
        if let Some((input, report)) = &self.detection_report {
            ui.separator();
            ui.label(egui::RichText::new("Auto-detect result — copy into Roblox").strong());
            for (i, slot) in report.slots.iter().enumerate().filter(|(_, s)| s.enabled) {
                ui.label(format!(
                    "Slot {} · {} · keys {}–{} · transpose {:+}",
                    i + 1,
                    slot.inst_type.label(),
                    slot.min_note,
                    slot.max_note,
                    slot.transposition
                ));
                let counts = report.after.slots[i];
                ui.small(format!(
                    "{} / {} triggered · {} original · {} octave-folded · {} skipped",
                    counts.played(),
                    counts.total,
                    counts.original,
                    counts.folded,
                    counts.skipped()
                ));
            }
            let before = report.before.notes;
            let after = report.after.notes;
            ui.label(format!(
                "Simulated coverage: {} / {} → {} / {} note-ons",
                before.played(),
                before.total,
                after.played(),
                after.total
            ));
            ui.label(format!(
                "{} original · {} octave-folded · {} skipped",
                after.original,
                after.folded,
                after.skipped()
            ));
            ui.small(format!(
                "Premature key cutoffs: {} → {} · transpose steps: {} → {}",
                report.before.cutoffs,
                report.after.cutoffs,
                report.before.steps,
                report.after.steps
            ));
            ui.small(format!(
                "{} · {} simulations · bounded best-effort search",
                input.path.file_name().unwrap_or_default().to_string_lossy(),
                report.simulations
            ));
            ui.small("Simulation estimates triggered notes; Roblox acceptance and timing may differ. Global settings were preserved.");
        }
    }

    fn disconnect_midi(&mut self) {
        invalidate_output(&self.shared_state);
        if let Some(connection) = self.connection.take() {
            let (input, _) = connection.close();
            self.midi_input = Some(input);
        }
    }

    fn connect_selected(&mut self) {
        if self.connection.is_some() {
            return;
        }
        if self.midi_input.is_none() {
            self.refresh_ports();
        }
        let Some((name, port)) = self
            .available_ports
            .iter()
            .find(|(name, _)| Some(name) == self.selected_port_name.as_ref())
            .cloned()
        else {
            return;
        };
        let Some(input) = self.midi_input.take() else {
            return;
        };
        invalidate_output(&self.shared_state);
        self.shared_state
            .device_state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pipeline
            .reset_source_state(MidiSource::Live);
        match input.connect(
            &port,
            "miditoroblox-in",
            |_stamp, message, shared| send_live_message(shared, message),
            self.shared_state.clone(),
        ) {
            Ok(connection) => {
                self.connection = Some(connection);
                self.status_message = format!("Connected to {name}");
            }
            Err(e) => {
                self.status_message = format!("Error connecting: {e}");
                self.midi_input = Some(e.into_inner());
            }
        }
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
                    }
                    Err(e) => {
                        self.status_message = format!("Failed to create MidiInput: {}", e);
                        return;
                    }
                }
            }
        };

        self.available_ports.clear();
        for port in midi_in.ports() {
            let name = midi_in
                .port_name(&port)
                .unwrap_or_else(|_| "Unknown".to_string());
            self.available_ports.push((name, port));
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

        if let Some(message) = self.midi_player.poll() {
            self.status_message = message;
        }
        if let Some(error) = self.midi_player.storage_error.take() {
            self.persistence_error = Some(error);
        }
        if let Some(error) = self
            .shared_state
            .error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            self.status_message = error;
        }
        let is_focused = ctx.input(|i| i.focused);
        let was_focused = self.shared_state.app_focused.load(Ordering::SeqCst);
        // Install the grace deadline before opening the focus gate.
        if !is_focused && was_focused {
            *self
                .shared_state
                .focus_ready_at
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Instant::now() + midi_player::FOCUS_DELAY;
        }
        self.shared_state
            .app_focused
            .store(is_focused, Ordering::SeqCst);
        let use_midi = self.shared_state.use_midi_device.load(Ordering::Relaxed);
        if is_focused && !was_focused {
            if !use_midi && self.midi_player.status() == PlaybackStatus::Playing {
                self.player_paused_by_focus = true;
                self.midi_player.pause();
            }
            invalidate_output(&self.shared_state);
        }
        if !use_midi {
            let status = self.midi_player.status();
            if is_focused && status == PlaybackStatus::Playing {
                self.midi_player.pause();
                self.player_paused_by_focus = true;
                self.status_message = "Paused: switch to Roblox to resume".into();
            } else if !is_focused && self.player_paused_by_focus && status == PlaybackStatus::Paused
            {
                self.midi_player.resume();
                self.player_paused_by_focus = false;
                self.status_message = "Playing (200 ms focus delay)".into();
            }
        }
        let is_panic = self.shared_state.panic_active.load(Ordering::SeqCst);
        if is_panic && !self.player_was_panic {
            self.midi_player.pause();
            self.player_paused_by_focus = false;
            self.status_message = "Panic: output stopped".into();
        }
        self.player_was_panic = is_panic;
        // Refresh focus and transport state even during rests, loading, or live input.
        let active = self.midi_player.status() == PlaybackStatus::Playing
            || self.midi_player.busy()
            || (use_midi && self.shared_state.visualizer_enabled.load(Ordering::Relaxed));
        ctx.request_repaint_after(Duration::from_millis(if active { 16 } else { 100 }));

        // Top panel
        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Miditoroblox");
                ui.add_space(20.0);

                // Left Side
                ui.horizontal(|ui| {
                    let mut use_midi = self.shared_state.use_midi_device.load(Ordering::Relaxed);
                    if ui.checkbox(&mut use_midi, "MIDI Device").changed() {
                        invalidate_output(&self.shared_state);
                        self.shared_state
                            .use_midi_device
                            .store(use_midi, Ordering::Relaxed);
                        if use_midi {
                            self.midi_player.stop();
                            self.player_paused_by_focus = false;
                            self.refresh_ports();
                            self.connect_selected();
                        } else {
                            self.disconnect_midi();
                            self.status_message = "MIDI device disabled".into();
                        }
                    }

                    if use_midi {
                        let selected_text = self
                            .selected_port_name
                            .clone()
                            .unwrap_or_else(|| "Select Port...".to_string());
                        egui::ComboBox::new("midi_selector_header", "")
                            .selected_text(selected_text)
                            .show_ui(ui, |ui| {
                                for (name, _) in &self.available_ports {
                                    if ui
                                        .selectable_label(
                                            self.selected_port_name.as_ref() == Some(name),
                                            name,
                                        )
                                        .clicked()
                                    {
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
                    if ui
                        .checkbox(&mut self.always_on_top, "Always On Top")
                        .changed()
                    {
                        let level = if self.always_on_top {
                            egui::WindowLevel::AlwaysOnTop
                        } else {
                            egui::WindowLevel::Normal
                        };
                        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
                    }
                    ui.add_space(10.0);
                    ui.label("Opacity:");
                    if ui
                        .add(
                            egui::Slider::new(&mut self.window_opacity, 0.1..=1.0)
                                .show_value(false),
                        )
                        .changed()
                    {
                        let mut regulars = egui::Visuals::dark();
                        let alpha = (self.window_opacity * 255.0) as u8;
                        regulars.window_fill = egui::Color32::from_black_alpha(alpha);
                        regulars.panel_fill = egui::Color32::from_black_alpha(alpha);
                        ctx.set_visuals(regulars);
                    }
                    ui.add_space(10.0);
                    let mut panic_en = self.shared_state.panic_enabled.load(Ordering::Relaxed);
                    if ui
                        .checkbox(&mut panic_en, "ALT to Panic")
                        .on_hover_text("Right ALT to Panic")
                        .changed()
                    {
                        self.shared_state
                            .panic_enabled
                            .store(panic_en, Ordering::Relaxed);
                    }
                });
            });
        });

        egui::TopBottomPanel::bottom("saved_data").show(ctx, |ui| {
            let store = self
                .midi_player
                .store
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            ui.small(format!("Saved data: {}", store.root.display()));
            if let Some(error) = &self.persistence_error {
                ui.colored_label(egui::Color32::YELLOW, error);
            }
        });
        // Bottom panels (Order: absolute bottom first, then stacked on it)
        egui::TopBottomPanel::bottom("footer").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let transpose = self.shared_state.transpose.load(Ordering::Relaxed);
                ui.label(egui::RichText::new(format!("In-Game Transpose: {transpose}")).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Reset All").clicked() {
                        invalidate_output(&self.shared_state);
                        if let Ok(mut state) = self.shared_state.device_state.lock() {
                            state.pipeline.solver.reset_transpose();
                        }
                        self.shared_state.transpose.store(0, Ordering::Relaxed);
                    }
                    ui.add_space(10.0);
                    if ui.button("Release All").clicked() {
                        invalidate_output(&self.shared_state);
                    }
                });
            });
        });

        egui::TopBottomPanel::bottom("visualizer_panel")
            .min_height(0.0)
            .show(ctx, |ui| {
                let vis_enabled = self.shared_state.visualizer_enabled.load(Ordering::Relaxed);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Log:").weak());
                    ui.label(&self.status_message);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let mut vis = self.shared_state.visualizer_enabled.load(Ordering::Relaxed);
                        if ui.checkbox(&mut vis, "Show Visualizer").changed() {
                            self.shared_state
                                .visualizer_enabled
                                .store(vis, Ordering::Relaxed);
                        }
                        if vis_enabled {
                            ui.separator();
                            egui::ComboBox::new("vis_mode", "")
                                .selected_text("Modes")
                                .show_ui(ui, |ui| {
                                    let mut sm = self
                                        .shared_state
                                        .visualizer_show_midi
                                        .load(Ordering::Relaxed);
                                    if ui.checkbox(&mut sm, "Midi In").changed() {
                                        self.shared_state
                                            .visualizer_show_midi
                                            .store(sm, Ordering::Relaxed);
                                    }
                                    let mut sr = self
                                        .shared_state
                                        .visualizer_show_roblox
                                        .load(Ordering::Relaxed);
                                    if ui.checkbox(&mut sr, "Roblox Out").changed() {
                                        self.shared_state
                                            .visualizer_show_roblox
                                            .store(sr, Ordering::Relaxed);
                                    }
                                });
                        }
                    });
                });

                if vis_enabled {
                    ui.separator();
                    let (response, painter) = ui.allocate_painter(
                        egui::vec2(ui.available_width(), 80.0),
                        egui::Sense::hover(),
                    );
                    let rect = response.rect;
                    let white_key_width = rect.width() / 52.0;
                    let black_key_width = white_key_width * 0.6;
                    let (white_key_height, black_key_height) = (rect.height(), rect.height() * 0.6);
                    let input_set = self
                        .shared_state
                        .active_notes
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .clone();
                    let output_set = self
                        .shared_state
                        .active_output_notes
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .clone();
                    let (show_input, show_output) = (
                        self.shared_state
                            .visualizer_show_midi
                            .load(Ordering::Relaxed),
                        self.shared_state
                            .visualizer_show_roblox
                            .load(Ordering::Relaxed),
                    );

                    let draw_key = |key_rect: egui::Rect, note: u8, is_black: bool| {
                        let inp = show_input && input_set.contains(&note);
                        let outp = show_output && output_set.contains(&note);
                        let base_color = if is_black {
                            egui::Color32::BLACK
                        } else {
                            egui::Color32::WHITE
                        };
                        let input_color = egui::Color32::GREEN;
                        let output_color = egui::Color32::from_rgb(0, 100, 255);
                        if inp && outp && show_input && show_output {
                            let half_h = key_rect.height() / 2.0;
                            painter.rect_filled(
                                egui::Rect::from_min_size(
                                    key_rect.min,
                                    egui::vec2(key_rect.width(), half_h),
                                ),
                                if is_black { 1.0 } else { 2.0 },
                                input_color,
                            );
                            painter.rect_filled(
                                egui::Rect::from_min_size(
                                    egui::pos2(key_rect.min.x, key_rect.min.y + half_h),
                                    egui::vec2(key_rect.width(), half_h),
                                ),
                                if is_black { 1.0 } else { 2.0 },
                                output_color,
                            );
                        } else if inp {
                            painter.rect_filled(
                                key_rect,
                                if is_black { 1.0 } else { 2.0 },
                                input_color,
                            );
                        } else if outp {
                            painter.rect_filled(
                                key_rect,
                                if is_black { 1.0 } else { 2.0 },
                                output_color,
                            );
                        } else {
                            painter.rect_filled(
                                key_rect,
                                if is_black { 1.0 } else { 2.0 },
                                base_color,
                            );
                        }
                        painter.rect(
                            key_rect,
                            1.0,
                            egui::Color32::TRANSPARENT,
                            egui::Stroke::new(1.0_f32, egui::Color32::GRAY),
                            egui::StrokeKind::Inside,
                        );
                    };

                    let mut x_pos = rect.min.x;
                    for note in 21..=108u8 {
                        if !matches!(note % 12, 1 | 3 | 6 | 8 | 10) {
                            draw_key(
                                egui::Rect::from_min_size(
                                    egui::pos2(x_pos, rect.min.y),
                                    egui::vec2(white_key_width, white_key_height),
                                ),
                                note,
                                false,
                            );
                            x_pos += white_key_width;
                        }
                    }
                    let mut white_key_idx = 0;
                    for note in 21..=108u8 {
                        if matches!(note % 12, 1 | 3 | 6 | 8 | 10) {
                            let center_x = rect.min.x + (white_key_idx as f32 * white_key_width);
                            draw_key(
                                egui::Rect::from_min_size(
                                    egui::pos2(center_x - (black_key_width / 2.0), rect.min.y),
                                    egui::vec2(black_key_width, black_key_height),
                                ),
                                note,
                                true,
                            );
                        } else {
                            white_key_idx += 1;
                        }
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
                if self.connection.is_some() {
                    ui.horizontal(|ui| {
                         ui.label(egui::RichText::new("Status: Connected").color(egui::Color32::GREEN));
                         if ui.button("Disconnect").clicked() {
                             self.disconnect_midi();
                             self.status_message = "Disconnected".into();
                             self.refresh_ports();
                         }
                    });
                    ui.separator();
                } else {
                     ui.label("Status: Not Connected");
                     let connect_enabled = self.selected_port_name.is_some();
                     if ui.add_enabled(connect_enabled, egui::Button::new("Connect")).clicked() {
                         self.connect_selected();
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
                            if ui.button("Add Dir").clicked()
                                && !self.new_dir_input.trim().is_empty() {
                                    self.midi_player.scan_dirs.push(midi_player::ScanDir {
                                        path: std::path::PathBuf::from(self.new_dir_input.trim()),
                                        recursive: self.new_dir_recursive,
                                    });
                                    self.new_dir_input.clear();
                                }
                        });

                        ui.horizontal(|ui| {
                            if ui.button("Scan Files").clicked() {
                                self.midi_player.scan();
                            }
                            ui.label(format!("Files found: {}", self.midi_player.midi_files.len()));
                            if self.midi_player.busy() { ui.spinner(); ui.label("Reading MIDI files…"); }
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
                                let is_playing = self.midi_player.playing_idx == Some(idx) && self.midi_player.status() != PlaybackStatus::Stopped;

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
                        if let Some(idx) = self.midi_player.selected_idx
                            && let Some(path) = self.midi_player.midi_files.get(idx) {
                                let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("Unknown");
                                ui.label(format!("Selected: {}", filename));
                            }

                        // Transport buttons
                        ui.horizontal(|ui| {
                            let status = self.midi_player.status();
                            let has_selection = self.midi_player.selected_idx.is_some();

                            if ui.add_enabled(has_selection || status == PlaybackStatus::Paused, egui::Button::new("▶ Play")).clicked() {
                                if status == PlaybackStatus::Paused && (!has_selection || self.midi_player.playing_idx == self.midi_player.selected_idx) {
                                    self.midi_player.resume();
                                } else if let Some(idx) = self.midi_player.selected_idx {
                                    self.midi_player.play(idx);
                                }
                                self.player_paused_by_focus = is_focused;
                            }

                            if ui.add_enabled(status == PlaybackStatus::Playing || self.player_paused_by_focus, egui::Button::new("⏸ Pause")).clicked() {
                                self.midi_player.pause();
                                self.player_paused_by_focus = false;
                            }

                            if ui.add_enabled(status != PlaybackStatus::Stopped || self.player_paused_by_focus || self.midi_player.is_loading(), egui::Button::new("⏹ Stop")).clicked() {
                                self.midi_player.stop();
                                self.player_paused_by_focus = false;
                            }
                        });

                        // 6. Progress / Seek Bar
                        let status = self.midi_player.status();
                        if status != PlaybackStatus::Stopped {
                            let pos = self.midi_player.position();
                            let total = self.midi_player.total();
                            if total > 0.0 {
                                let mut slider_val = self.player_seek_pos.unwrap_or(pos);
                                let resp = ui.add(egui::Slider::new(&mut slider_val, 0.0..=total).show_value(false));
                                if resp.dragged() {
                                    self.player_seek_pos = Some(slider_val);
                                }
                                if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
                                    self.midi_player.seek(self.player_seek_pos.take().unwrap_or(slider_val));
                                }
                                ui.label(format!("{} / {}", format_time(pos), format_time(total)));
                            }
                        }
                    });
                    ui.separator();
                    }

                    if !self.shared_state.use_midi_device.load(Ordering::Relaxed) { self.editor_ui(ui); }
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
                            let mut accuracy = self.shared_state.solver_accuracy.load(Ordering::Relaxed);
                            if ui.add(egui::Slider::new(&mut accuracy, 0..=100).text("Speed ↔ Accuracy")).changed() {
                                self.shared_state.solver_accuracy.store(accuracy, Ordering::Relaxed);
                            }
                            ui.label(format!("Maximum jump: {} semitones", solver::jump_budget(accuracy)))
                                .on_hover_text("Speed may skip notes requiring larger jumps. Played notes keep their intended pitch. Accuracy allows more transpose steps.");
                            let mut rng = self.shared_state.transpose_range.load(Ordering::Relaxed);
                            if ui.add(egui::Slider::new(&mut rng, 12..=24).text("Transpose limit")).changed() { self.shared_state.transpose_range.store(rng, Ordering::Relaxed); }
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
                    let mut range_inst = self.shared_state.range_instrument_enabled.load(Ordering::Relaxed);
                    if ui.checkbox(&mut range_inst, "Range Instrument Support (Experimental)").changed() {
                        self.shared_state.range_instrument_enabled.store(range_inst, Ordering::Relaxed);
                    }
                    if range_inst {
                        ui.indent("range_inst_slots", |ui| {
                            let mut slots = *self.shared_state.instrument_slots.lock().unwrap_or_else(|e| e.into_inner());
                            for (i, slot) in slots.iter_mut().enumerate() {
                                ui.group(|ui| {
                                    ui.horizontal(|ui| {
                                        ui.checkbox(&mut slot.enabled, format!("Slot {}", i + 1));
                                        if slot.enabled {
                                            egui::ComboBox::new(format!("inst_type_{}", i), "Type")
                                                .selected_text(slot.inst_type.label())
                                                .show_ui(ui, |ui| {
                                                    for t in InstrumentType::all() {
                                                        ui.selectable_value(&mut slot.inst_type, *t, t.label());
                                                    }
                                                });
                                        }
                                    });
                                    if slot.enabled {
                                        ui.horizontal(|ui| {
                                            ui.label("Range:");
                                            ui.add(egui::Slider::new(&mut slot.min_note, 1..=88).text("Min"));
                                            ui.add(egui::Slider::new(&mut slot.max_note, 1..=88).text("Max"));
                                        });
                                        if slot.min_note > slot.max_note {
                                            slot.min_note = slot.max_note;
                                        }
                                        ui.horizontal(|ui| {
                                            ui.label("Transpose:");
                                            ui.add(egui::Slider::new(&mut slot.transposition, -24..=24));
                                        });
                                    }
                                });
                            }
                            *self.shared_state.instrument_slots.lock().unwrap_or_else(|e| e.into_inner()) = slots;
                            self.detection_ui(ui);
                        });
                    }

                });
            }
        });
        self.midi_player.inspect_selection();
        self.poll_detection();
        self.save_preferences(false);
    }
}

impl Drop for MidiApp {
    fn drop(&mut self) {
        self.save_preferences(true);
        if let Some(job) = self.detection.take() {
            job.cancel.store(true, Ordering::Relaxed);
            drop(job);
        }
        self.shared_state.shutdown.store(true, Ordering::SeqCst);
        self.connection = None;
        self.midi_player.shutdown();
        invalidate_output(&self.shared_state);
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn format_time(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    if total >= 3600 {
        format!("{}:{:02}:{:02}", total / 3600, total / 60 % 60, total % 60)
    } else {
        format!("{}:{:02}", total / 60, total % 60)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "linux")]
    // Keep the existing X11 window behavior, including Always On Top.
    unsafe {
        std::env::remove_var("WAYLAND_DISPLAY")
    };
    solver::get_available_mappings();
    let device = platform::build_keyboard()?;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_transparent(true)
            .with_inner_size([1000.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Miditoroblox",
        options,
        Box::new(|cc| Ok(Box::new(MidiApp::new(cc, device)))),
    )
    .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn time_labels_use_minutes_and_hours() {
        assert_eq!(format_time(0.0), "0:00");
        assert_eq!(format_time(125.9), "2:05");
        assert_eq!(format_time(3661.0), "1:01:01");
    }
}
