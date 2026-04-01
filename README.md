# WARNING!!!

This program automates keyboard inputs, which can lead to unexpected behavior like opening programs or clicking things while it recieves MIDI signals and you have another program focused, please use the right ALT panic mode!!!

This program works only on Linux, and if it doesn't work you might have to run it with sudo. Sometimes there may be problems with fast keys clicking, I suspect that is because of the kernel but I have no idea (works perfectly on my end).

## MIDI to Roblox

This Rust Program maps MIDI notes to Roblox keys. It uses /dev/uinput to create a virtual keyboard and uses it to play the notes from a MIDI signal. It features a GUI to control the program.

Building:

### 1.
`Clone the repository`

### 2.
`cd into the repository`

### 3.
`cargo run --release`

Usage:

Select a midi device that should be used by the program, then click the "Connect" button.