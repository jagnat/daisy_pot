# Timing: budgets and how to measure them

The goal is to replace "it sounds fine" with a number that is either inside a budget or
outside it. Every milestone in `ROADMAP.md` has an exit criterion expressed in terms
defined here.

## The budget

At sysclk **96 MHz** and **44.1 kHz** sample rate:

| Quantity | Value |
|---|---|
| Cycles per stereo frame | 96e6 / 44100 = **2,177** |
| Block size | 64 frames |
| Cycles per block | **139,320** |
| Wall time per block | **1.451 ms** |
| Target audio load | 60% → **~1,306 cycles/frame** for DSP |
| At 8 voices | **~163 cycles/voice/sample** |

That last number is the one to keep in your head. It governs every DSP design decision:
a per-sample `f32` divide is ~14 cycles on the M4F FPU, a `powf` via `micromath` is far
more. Anything transcendental belongs at control rate (1 kHz), not audio rate.

Display, for contrast:

| Quantity | Value |
|---|---|
| Framebuffer | 12,000 bytes |
| Full frame on the wire | ~12,482 bytes ≈ **50 ms** @ 2 MHz |
| One line | 52 bytes ≈ **208 µs** |
| Max full-frame rate | ~20 Hz |

## Layer 1: underrun counter (ground truth)

The only measurement that cannot be argued with. With circular double-buffered DMA:

```rust
static UNDERRUNS: AtomicU32 = AtomicU32::new(0);
```

Keep a `filled: [bool; 2]`. The fill routine sets `filled[half] = true`. The DMA
half/full-transfer ISR checks whether the half the hardware is *about to play* was
actually filled; if not, increment `UNDERRUNS` and (in debug) fill it with silence.

**This is the acceptance criterion for every audio milestone: 0 underruns over 10
minutes.** A load meter can be wrong; a zero underrun count over 26 million frames
cannot.

## Layer 2: DWT cycle counter (headroom)

The underrun counter tells you when you fell off the cliff. DWT tells you how far you
are from the edge. Cortex-M4 has a free-running cycle counter in the DWT unit:

```rust
let mut core = cortex_m::Peripherals::take().unwrap();
core.DCB.enable_trace();
core.DWT.enable_cycle_counter();
// then, anywhere:
let t = cortex_m::peripheral::DWT::cycle_count();
```

At 96 MHz one cycle is 10.4 ns and the counter wraps every ~44.7 s, so always compare
with `wrapping_sub`.

In the audio ISR, timestamp entry and exit, and maintain three statistics:

- **running max** — worst case observed, the number that matters for a hard deadline
- **EWMA** — typical load, for spotting drift
- **max / 139,320** — a DAW-style CPU percentage

Report these from the `main` idle loop via `defmt` once per second. Never format `defmt`
inside the audio ISR itself; store to an atomic and print from thread mode.

Also worth timestamping: the delta between consecutive ISR *entries*. It should be
exactly 139,320 cycles. The variance is your interrupt jitter, and it will tell you
immediately when something at another priority is blocking you.

## Layer 3: scope pin (external truth)

The only measurement whose overhead is not itself part of the measurement. One GPIO,
high on ISR entry, low on exit. An RAII guard makes it impossible to forget the low:

```rust
pub struct ScopeGuard<'a, P: OutputPin>(&'a mut P);

impl<'a, P: OutputPin> ScopeGuard<'a, P> {
    pub fn new(pin: &'a mut P) -> Self { let _ = pin.set_high(); Self(pin) }
}

impl<'a, P: OutputPin> Drop for ScopeGuard<'a, P> {
    fn drop(&mut self) { let _ = self.0.set_low(); }
}
```

A logic analyzer then gives you duty cycle (= CPU load) and pulse-to-pulse jitter
directly, including any time spent in a higher-priority handler you forgot about. Use a
second pin for the display DMA so you can see the two paths interleave, and confirm
visually that display work never delays audio.

## Layer 4: host tests and WAV rendering (correctness)

Timing can be perfect while the sound is wrong. Because `dsp` compiles for the host:

- **Unit tests** for wavetable interpolation, envelope stage transitions, filter
  coefficients, voice stealing. Plain `cargo test`, no hardware.
- **WAV render**: run the engine for 10 s of simulated time, write a `.wav`, then check
  for DC offset, discontinuities at parameter changes (the zipper-noise regression), and
  aliasing via FFT. This is the "verify by eye on a spectrogram instead of by ear" tool.
- **Golden-file tests**: render a fixed parameter sequence and assert the output hash or
  RMS envelope matches. Catches accidental DSP regressions during refactors.

## Determinism notes

- **Set FZ (flush-to-zero) in FPSCR.** Denormal handling on the M4F FPU costs extra
  cycles, so a decaying envelope tail can quietly become your worst case. FZ makes float
  op timing constant.
- **No allocation, ever.** There is no allocator; keep it that way.
- **`opt-level = 's'` is set for both profiles.** Good default, but when you profile a
  hot DSP loop, check `opt-level = 3` on the `dsp` crate specifically — the tradeoff for
  audio inner loops usually favors speed, and you have 512K of flash.
- **Stack.** `flip-link` is already enabled, so stack overflow into statics faults
  instead of corrupting. Add `cargo-call-stack` at M3 when the voice graph gets deep.

## Where the numbers change

If you move to 48 kHz: 2,000 cycles/frame exactly, which is a nicer number and often has
cleaner I2S divisors than 44.1 kHz. Worth reconsidering at M1 while the change is cheap.
If you port to Daisy (STM32H750 @ 400+ MHz), every budget here multiplies by ~4 — but the
*structure* of the measurement does not change, which is the point.
