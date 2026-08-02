# daisy_pot

A hardware audio sequencer / synth on a Daisy Seed3. Sound design and machine design are
both still open questions — this is an instrument being figured out by building it, not a
spec being implemented.

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

- Daisy Seed3, STM32H750IBK6 Cortex-M7 @ 480 MHz. Reference manual RM0433.
- 128K internal flash — the tight resource. 1M SRAM, 64M SDRAM, 8M QSPI flash.
- Onboard TAC5242 codec on SAI1, configured over I2C. Up to 32-bit / 192 kHz. Its output
  is a TPA6110A2 headphone amp at 1 Vrms — line level, rated for 16–32 Ω.
- MAX98357A on SAI2 drives a 4 Ω speaker. Class-D with an integrated DAC, so it takes I2S
  directly. Independent of SAI1, so speaker and line out can run at once.
- 360° pot read as two phase-shifted triangle waves on the ADC (16-bit here).
- Sharp memory LCD (LS027B7DH01), 400x240 1bpp, for the UI.
- Pots and buttons. SD via SDMMC1 for sample banks eventually.
- Flashed over USB DFU: hold BOOT, tap RESET, `cargo run`. No debug probe attached, so
  there is no RTT and no `defmt` — the LED is currently the only output channel.

### Pin map

| Function | Pin | Header | Peripheral | Status |
|---|---|---|---|---|
| User LED | PC7 | not broken out | GPIO | in use |
| Pot 1 | PC0 | 22 (A0) | ADC1_INP10 | planned |
| Pot 2 | PA3 | 23 (A1) | ADC1_INP15 | planned |
| Vol pot | PB1 | 24 (A2) | ADC1_INP5 | planned |
| Amp DIN | PA0 | 32 (A10) | SAI2_SD_B, AF10 | planned |
| Amp LRCLK | PG9 | 34 (D27) | SAI2_FS_B, AF10 | planned |
| Amp BCLK | PA2 | 35 (A11) | SAI2_SCK_B, AF8 | planned |
| Display CS | PG10 | 8 (D7) | GPIO, active high | planned |
| Display SCK | PG11 | 9 (D8) | SPI1_SCK, AF5 | planned |
| Display MOSI | PB5 | 11 (D10) | SPI1_MOSI, AF5 | planned |
| Button | PB14 | 36 (D29) | GPIO, pull-up | planned |

The onboard codec is on SAI1 with its I2C bus, both internal to the module. SAI2 sub-block
B is the only complete I2S transmitter on the header. Sub-block A can't run standalone —
its SCK/FS aren't broken out — but it can run synchronous to B, putting a second stereo
pair on SD_A (PD11, header 33) sharing B's clocks.

Pot 1 and 2 are on both ADC1 and ADC2, so dual regular simultaneous mode is available if
the two phases ever need sampling at the same instant. Vol is ADC1/2 as well; all three
share one ADC1 scan today.

Reserved, don't allocate: header 2–7 (D1–D6) is the full SDMMC1 4-bit interface; 14–15
(D13/D14) is USART1, the DIN MIDI port; 12–13 (D11/D12) is I2C1, the only I2C out.
PB14/PB15 are USB_HS D-/D+ — the button costs the second USB port. DFU is unaffected: the
onboard USB-C is OTG_FS on PA11/PA12, which is not broken out.

### Power and ground

- Pots run off +3V3A (21) / AGND (20). Same node as the ADC reference, so rail noise
  cancels ratiometrically. Display runs off +3V3D (38) / DGND (40) to keep SPI switching
  current off the analog rail.
- **AGND must be tied to DGND externally** — they are not joined on the module, and the
  datasheet warns of noise or damage. One tie point, near the module; two makes a loop.
- The amp needs its own supply. VIN (39) is diode-OR'd with USB VBUS into an internal
  node, so it cannot source 5 V back out, and the USB-C is a 500 mA default-power sink
  either way. MAX98357A VDD is 2.5–5.5 V: 3.3 V yields ~1.4 W into 4 Ω, 5 V ~3.2 W.
- MAX98357A `SD` ties to 3V3; ground is shutdown. It needs no MCLK, recovering clock from
  BCLK at 32/48/64 × LRCLK.

## Where it's going

- **RTIC or Embassy** is on the table and not a big deal to adopt if the project actually
  grows a task structure that wants it. Bare interrupt handlers until then.
- Sequencer, sample playback, more voices — in some order, decided later.
- The 64M SDRAM makes minutes-long delay lines and granular buffers possible, which the
  instrument has never had room for before.

<!-- Below here: additions I didn't ask for, cut freely -->

## Suggestions

- **Audio is the one hard deadline.** Everything else — UI, controls, sequencing — is
  soft. When something has to give, it's not the audio ISR. Worth treating as the
  standing tiebreaker rather than re-deciding each time.
- **Synthesis code stays free of HAL types.** A `fill()` that only touches `f32` and
  slices is portable and host-testable; one that takes a `Transfer` is neither. Costs
  nothing to hold to now.
- **The PAC can't compile for macOS** — LLVM rejects its interrupt vector section as
  invalid Mach-O. So `cargo test` on the host is impossible for anything sharing a crate
  with the HAL. Relevant if host-testing DSP ever sounds appealing; it's the reason it
  would need a separate crate.
- **DMA1/DMA2 cannot reach DTCM or ITCM.** Buffers go in AXI SRAM, which is where
  `memory.x` points `RAM`. The failure looks exactly like a misconfigured stream.
- **Keep the pin map current in this file.** It's the thing I'll otherwise guess wrong
  about, and it's cheap to keep accurate.
