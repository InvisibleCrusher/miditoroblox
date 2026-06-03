# WARNING!!!

This program automates keyboard inputs, which can lead to unexpected behavior like opening programs or clicking things while it recieves MIDI signals and you have another program focused, please use the right ALT to stop the program from sending key inputs.

This program works only on Linux and Windows, if it doesn't work you might have to run it with sudo / administrator privileges.

## MIDI to Roblox

This Rust Program maps MIDI notes to Roblox keys. On linux it uses /dev/uinput to create a virtual keyboard and uses it to play the notes from a MIDI signal or file. It features a GUI to control the program.

Building:

### 1.
`Clone the repository`

### 2.
`cd into the repository`

### 3.
`cargo run --release`
