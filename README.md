# MIDI to Roblox

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

For the program to play anything, you have to at least enable "Enable Base (C2-C7)".
That will enable the program to use the base range of the keyboard used by most games (currently the range is hardcoded).
Some games also enable using CTRL with more keys to extend the range and that is available as "Enable Low Range" and "Enable High Range".
If the notes coming from the MIDI device are outside of the scale used by the program, you can enable "Enable Auto-Octave Transposition" to automatically transpose the notes to fit the scale.
There is also an Experimental setting for a different method for playing black keys. In games that allow transposing the keyboard by using the up and down arrow keys, this option allows black keys to be held down.