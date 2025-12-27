# WARNING!!!

This program automates keyboard inputs, which can lead to unexpected behavior like opening programs or clicking things while it recieves MIDI signals and you have another program focused!!!

This program also works only on Linux, and if it doesn't work you might have to run it with sudo.

## MIDI to Roblox

This Rust Program maps MIDI notes to Roblox keys. It uses /dev/uinput to create a virtual keyboard and uses it to play the notes from a MIDI signal. It features a simple GUI to control the program.

Building:

### 1.
`Clone the repository`

### 2.
`cd into the repository`

### 3.
`cargo build --release` (or you can just use cargo run --release to build AND run)

### 4.
`cargo run --release` 

Usage:

Select a midi device that should be used by the program, then click the "Connect" button.

The experimental setting is for a different method for playing black keys. In games that allow transposing the keyboard by using the up and down arrow keys, this option allows black keys to be held down.