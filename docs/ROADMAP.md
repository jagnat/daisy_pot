# Roadmap

Each milestone has a **binary exit criterion**. Do not start the next one until the
current one's criterion is met and measured, not estimated.

Terms (`UNDERRUNS`, load meter, WAV render) are defined in `TIMING.md`. The task table
and shared-state patterns are in `ARCHITECTURE.md`.

---

## M0 — Workspace split

Restructure only. No behavior change, no new hardware.

- Create `[workspace]` root; move DSP into `dsp/`, firmware into `firmware/`.
- Move `SINE` / `SQUARE` / `SAW`, `pot_vol_to_linear`, and the phase-accumulator
  interpolation out of `main.rs` into `dsp` — they are already pure functions.
- Add `dsp/tests/` with the first host tests: wavetable interpolation at known phases,
  `pot_vol_to_linear` endpoints (note the current `val == 0.0` special case and the
  unused `min_raw`/`max_raw` bindings — decide what the curve should actually be and
  test that).
- Firmware still uses the blocking `block!(i2s_transfer.write(...))` loop.

**Exit:** `cargo test -p dsp` passes on the host; firmware builds and produces the same
sound as today.

**Why first:** every later milestone's verification depends on `dsp` being host-testable.
Doing this after the DMA work means untangling HAL types out of DSP code under pressure.

---

## M1 — DMA audio + instrumentation

The instrumentation *is* the milestone. Keep the oscillator exactly as it is.

- Circular DMA on SPI2 TX (DMA1 Stream 4 Ch 0 — verify against RM0383 Table 27).
- One buffer, `BLOCK * 2 ch * 2 halves` of `i16`, `BLOCK = 64`.
- Half-transfer and transfer-complete interrupts fill the opposite half.
- `IsrCell` for the engine; `DMA1_STREAM4` at NVIC priority `0 << 4`.
- Build `instr.rs`: `UNDERRUNS` counter, DWT entry/exit timestamps, running max + EWMA,
  ISR-entry-delta jitter, reported from the idle loop via `defmt` at 1 Hz.
- Add the scope pin and RAII guard. Confirm on a logic analyzer that the numbers the
  firmware reports match what the pin shows.
- Set FZ in FPSCR.
- Decide 44.1 vs 48 kHz now, while it is a one-line change.

**Exit:** 0 underruns over 10 minutes; load meter reports < 20%; scope duty cycle agrees
with the reported load within a few percent.

---

## M2 — Task model and control path

- `TIM2` at 1 kHz, NVIC priority `1 << 4`.
- ADC1 circular scan DMA (DMA2 Stream 0 Ch 0), TIM2-triggered, into `[u16; N]`. Delete
  the blocking `adc.convert()` calls.
- Debounce state machine for buttons at control rate, replacing the sample-counting
  `DEBOUNCE_SAMPLES` logic.
- One-pole smoothing on every pot value.
- Seqlock-published `ParamSnapshot` read by the audio ISR. No critical sections in the
  audio path.

**Exit:** still 0 underruns over 10 minutes; measured control latency (button edge to
audible change, on the scope) under 5 ms; WAV render of a parameter sweep shows no
zipper-noise discontinuities.

---

## M3 — Real voice architecture

All in `dsp`, all host-tested before it ever runs on hardware.

- `Voice` struct: oscillator + ADSR + filter.
- Polyphonic allocator with a defined voice-stealing rule.
- Mixer with headroom management.
- Golden-file regression tests; FFT check for aliasing.
- Run `cargo-call-stack` to confirm stack depth against the `flip-link` guard.

**Exit:** 8 voices simultaneously with load meter under 60% and 0 underruns; WAV render
clean (no DC offset, no clicks on note-on/note-off, aliasing below target).

---

## M4 — Sharp LS027 display

- SPI1 on **DMA2** Stream 3 Ch 3 — deliberately a different DMA controller from audio.
- Remember: **CS active HIGH**, **LSB-first**, ~2 MHz max SCK.
- `EXTCOMIN` driven by a hardware PWM timer channel, so VCOM toggling cannot be starved.
- 12,000-byte framebuffer plus a `[u8; 30]` dirty-line bitmask. DMA changed lines only —
  a full frame is ~50 ms and cannot meet 30 Hz.
- `embedded_graphics::DrawTarget` impl over the framebuffer.
- `TIM3` at 30 Hz, NVIC priority `2 << 4`.
- Second scope pin on the display path; confirm visually that display work never delays
  the audio pin.

**Exit:** 30 Hz UI redraw with 0 audio underruns over 10 minutes; audio load meter
unchanged from M3 within measurement noise.

---

## M5 — Sequencer

- Clock derived from the **audio sample counter**, not a wall-clock timer, so events land
  sample-accurately inside a block rather than quantized to block boundaries.
- Event queue consumed by the audio ISR at block start, split-block if an event falls
  mid-block.
- Pattern storage, tempo, swing.

**Exit:** rendered WAV shows note onsets at exactly the expected sample offsets across a
range of tempos, including tempos that do not divide evenly into the block size.

---

## M6 — Pick one

By here the foundation is done and these are genuinely independent:

- **Sampler + flash storage** — wear-leveled sample storage in the 512K flash, streaming
  playback.
- **Faust integration** — use `faust -lang rust`, not C FFI. The Rust backend avoids a
  second toolchain and ABI, and by now you have a stable engine interface to generate
  into. Deferred to here deliberately: doing it earlier costs the Rust practice that is
  the point of the project.
- **Daisy port (STM32H750)** — only `firmware/` is rewritten if `dsp` stayed HAL-free.
  All timing budgets multiply by ~4; the measurement structure is unchanged.

---

## Standing rules

- Update the concurrency table in `ARCHITECTURE.md` **before** changing task structure.
- Any milestone that touches audio re-runs the 10-minute underrun test.
- If `dsp` needs a HAL type, the boundary is wrong — pass a `Copy` struct instead.
- Revisit the RTIC migration trigger (end of `ARCHITECTURE.md`) at the end of M2 and M4.
