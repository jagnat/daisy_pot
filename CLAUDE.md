# synth_pill

A hardware audio sequencer / synth on an STM32F411 black pill. Sound design and machine
design are both still open questions — this is an instrument being figured out by
building it, not a spec being implemented.

## How I want to work on this

**I write the code.** The point of this project is getting fluent in Rust and in reading
embedded crates — HAL sources, PACs, reference manuals. Handing me finished code takes
that away. Default to explaining the shape of a thing, the API surface, the trade-off, or
where in a crate to look. I'll ask outright when I want something written, and I do
sometimes, for good reasons.

**Architecture evolves organically.** I want the architecture to get better, but by
noticing a real problem and fixing it, not by planning ahead of the code. Suggest
structure when the current structure is actually hurting. Don't propose a refactor for a
file that doesn't exist yet.

**Don't create documentation files.** No new markdown, no docs/, no design writeups
unless I ask for one by name. Answer in the conversation. Facts that need to persist go
in a comment next to the code they're about, per my global CLAUDE.md.

**Pointers to things I don't know about are welcome** — hardware timers, DMA modes, a
crate that solves it — as long as they stay short. A paragraph, not a document.

## Layout

Flat. `src/*.rs` with `mod foo;` in `main.rs`. New driver, new file, one line. Don't
propose workspaces, subdirectories, or crate splits without a concrete reason that exists
today.

## Hardware

- STM32F411CE black pill, 96 MHz
- I2S audio out on SPI2, 16-bit stereo, DMA
- 360° pot read as two phase-shifted triangle waves on the ADC
- Sharp memory LCD (LS027B7DH01), 400x240 1bpp, for the UI
- Pots and buttons
- SD / flash readers on hand for sample banks eventually

## Where it's going

- **STM32H7 port** when the Daisy Seed 3 boards arrive. Likely keeping an F411 build too,
  so avoid baking F411-specific assumptions into anything that isn't a driver.
- **RTIC or Embassy** is on the table and not a big deal to adopt if the project actually
  grows a task structure that wants it. Bare interrupt handlers until then.
- Sequencer, sample playback, more voices — in some order, decided later.

<!-- Below here: additions I didn't ask for, cut freely -->

## Suggestions

- **Audio is the one hard deadline.** Everything else — UI, controls, sequencing — is
  soft. When something has to give, it's not the audio ISR. Worth treating as the
  standing tiebreaker rather than re-deciding each time.
- **Synthesis code stays free of HAL types.** Falls out of the H7 port goal on its own: a
  `fill()` that only touches `f32` and slices ports for free, one that takes a `Transfer`
  doesn't. Costs nothing to hold to now.
- **The PAC can't compile for macOS** — LLVM rejects its interrupt vector section as
  invalid Mach-O. So `cargo test` on the host is impossible for anything sharing a crate
  with the HAL. Relevant if host-testing DSP ever sounds appealing; it's the reason it
  would need a separate crate.
- **Keep the pin map current in this file.** It's the thing I'll otherwise guess wrong
  about, and it's cheap to keep accurate.
