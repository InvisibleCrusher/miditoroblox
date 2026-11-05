# MIDI to Roblox

A Rust desktop app that converts a live MIDI input or MIDI file into keyboard notes for Roblox. Supports Linux and Windows.

The app sends keyboard input to the focused window. Right Alt stops output when **ALT to Panic** is enabled; use **Continue** to clear panic before playing again. Focusing this app pauses file playback and releases held keys. Switching back to Roblox resumes after a 200 ms delay so the first notes reach the game after the window switch. Live input uses the same focus delay.

File playback shows elapsed and total time as `minutes:seconds` (or `hours:minutes:seconds` for longer songs). The progress bar seeks in seconds, accounts for tempo changes, and advances through silent passages. Seeking and resuming restore MIDI programs, sustain state, and notes spanning the playback position.

Settings and app preferences save automatically after 500 ms without a change, with a final save on shutdown. Saved data lives in **`miditoroblox/` inside the working directory used to launch the app**, shown below the Log. `miditoroblox/settings.json` contains global playback settings (including instrument slots), scan folders, window/visualizer preferences, source mode, and the preferred MIDI port. Startup restores preferences and scans saved folders, but never starts playback or connects a MIDI device automatically. Runtime transpose, panic activation, held notes, and playback position are not saved. Malformed or unsupported settings files are preserved and reported; saving that file remains disabled until the file is repaired or removed and the app is restarted. Write failures are reported and retried.

Select a MIDI to load its **Instrument editor** without playing it. Each row represents one instrument sound within one track, with its General MIDI name/program, channels, and note count. Mute rows independently, or use **Mute all** / **Unmute all**. The same instrument in different tracks has separate rows; program changes on a shared channel are recognized across tracks. Channel 10 percussion has one row per track (individual drums and MIDI banks are not separated).

Only mutes are saved per MIDI, in `miditoroblox/midi/<SHA-256>.json`. Identical file contents share a profile even after copying, moving, or renaming; changed contents start a fresh profile. Original MIDI files are never modified and no sidecars are written next to them. Editing the loaded song releases output and rebuilds playback at its current position, retaining its transport/focus state. Auto-detect uses the same filtered timeline as playback, so muted notes do not affect its search or statistics. Changing a mute cancels outdated detection results. With everything muted, detection reports that there are no unmuted notes and keeps settings unchanged.

**Black Keys via Transpose** always keeps the optimized behavior: transpose remains at its current value until another note requires a change. Range Instrument Support remains available for routing; automatic drum switching and mouse automation have been removed.

**Smart Solver** processes each incoming note immediately, for live MIDI and MIDI file playback, without prediction or lookahead. With **Range Instrument Support**, candidate piano keys stay inside the matched slot’s Min/Max range. Free keys and keys already playing the same pitch are preferred over cutting off another note; unavoidable key theft favors the oldest held note.

The **Speed ↔ Accuracy** slider controls the maximum transpose jump per note: Speed allows 3 semitones, Accuracy allows 48, with intermediate values shown below the slider. Speed may skip notes that need larger jumps; notes that are played keep their intended pitch. **Transpose limit** restricts the absolute global transpose, and enabled instrument slots also constrain the combined transpose.

With **Range Instrument Support**, **Auto-detect settings** scans the selected MIDI using the enabled slots and their instrument types. Enabled types must be unique. It chooses separate, contiguous ranges covering keys 1–88, searches every slot order and transpose from −24 to +24, then simulates and refines promising layouts using the current playback settings. The search is deterministic and bounded to three refinement passes or 1,000 full simulations; it is best effort rather than a guaranteed optimum. It prioritizes triggered note count, then original pitches, fewer premature key cutoffs, fewer global transpose steps, and smaller slot transposes. Repeated note-ons count separately; velocity-zero note-ons are releases.

Detection runs in the background, supports cancellation, and sends no keyboard input. Changing the selected file, slot setup, or playback settings invalidates the pending result. Completed results apply ranges and transposes together and show overall/per-slot simulated coverage, original and octave-folded notes, skips, cutoffs, and the settings to copy into Roblox. Configure those ranges and transposes in Roblox manually; game timing and note acceptance can differ from simulation. Global playback settings are preserved. Empty, unmatched, and invalid files leave settings unchanged.

**Fold out-of-range notes by octaves**, off by default, moves a note to the nearest octave equivalent inside its matched slot’s range. This preserves pitch class, compensates for the slot transpose, and works for live input and MIDI files. A narrow range without an equivalent pitch still skips the note. Typed slots are tried before the existing **Other** fallback. Auto-detect simulation honors this toggle. Manual overlapping slot ranges remain supported.

After 300 ms without a new note-on, Smart Solver gently returns toward zero, one semitone every 100 ms. It can return while notes are held because Roblox keeps their original sounding pitches. Controllers and note-offs do not postpone the return; even a skipped incoming note restarts the grace period. Return steps use neutral modifiers and restore the held modifier state. Focus, panic, source changes, and disabling Smart Solver cancel pending return work.

To build and run:

```sh
cargo run --locked --release
```

The repository pins the Rust toolchain in `rust-toolchain.toml` and includes `Cargo.lock` for reproducible dependencies.

On Linux, install the development dependencies used by the GUI and MIDI backend:

```sh
sudo apt-get install libx11-dev libxcb1-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libasound2-dev libudev-dev
```

Linux keyboard output requires write access to `/dev/uinput`. The Right Alt monitor requires read access to keyboard devices under `/dev/input`; an unavailable monitor is reported in the status display. The app retains its existing X11 window backend. Windows uses the standard keyboard input API and reports injection failures in the status display.

Scans and MIDI loading run in the background. Unreadable folders, invalid files, and output failures appear in the existing Log display.

Validation:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```
