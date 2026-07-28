# Architecture

Target: STM32F411CE @ 96 MHz sysclk, 128K RAM, 512K flash.
Audio: I2S master on SPI2, 16-bit stereo, 44.1 kHz.
Concurrency: bare `cortex-m-rt` interrupt handlers with NVIC priorities. No RTOS, no
framework. See "When to migrate to RTIC" at the bottom.

## Crate layout

```
synth_pill/
├── Cargo.toml          # [workspace]
├── dsp/                # no_std. NO hal, NO cortex-m, NO defmt-in-hot-path.
│   ├── src/lib.rs      # oscillators, envelopes, filters, voice alloc, mixer
│   └── tests/          # host-side cargo test + WAV render
└── firmware/
    ├── src/main.rs     # clock init, peripheral setup, ISR definitions
    ├── src/audio.rs    # I2S + DMA, block fill, underrun accounting
    ├── src/control.rs  # ADC DMA, debounce, parameter smoothing
    ├── src/display/    # Sharp LS027 driver, DrawTarget impl
    └── src/instr.rs    # DWT cycle counters, scope pins, stats
```

The load-bearing rule: **`dsp` must compile for the host.** It depends only on `core`
(plus `micromath` if needed, which is `no_std` and host-compatible). No peripheral types
in any DSP signature. This buys three things at once:

- `cargo test` on the Mac, no flashing
- Offline WAV rendering, so correctness is verified by inspection rather than by ear
- A tractable Daisy (STM32H750) port later — only `firmware/` gets rewritten

If you ever find yourself wanting to pass a HAL type into `dsp`, that is the signal that
the boundary is in the wrong place. Pass a plain `Copy` struct instead.

## Concurrency table

This table is the design. Write changes here before changing code.

| Task | NVIC prio | Trigger | Deadline | Owns exclusively | Reads |
|---|---|---|---|---|---|
| `DMA1_STREAM4` | 0 (highest) | I2S TX half/full xfer | 1.451 ms **hard** | audio buffer, `dsp::Engine` | `ParamSnapshot` |
| `TIM2` | 1 | 1 kHz timer | 1 ms soft | debounce state, smoothers | ADC DMA buffer |
| `TIM3` | 2 | 30 Hz timer | 33 ms soft | framebuffer, SPI1 | `UiState` |
| `DMA2_STREAM3` | 2 | display SPI TX done | — | SPI1 line buffer | — |
| `main` idle loop | — (thread) | — | — | defmt reporting | stats counters |

### NVIC priority gotcha

On Cortex-M, **lower number = higher priority**. STM32F4 implements only the **top 4
bits** of the 8-bit priority register, so `NVIC::set_priority` values that differ in the
low nibble are *the same priority*. Setting `0` and `1` gives you two tasks at equal
priority that cannot preempt each other, silently.

Always space priorities by 16:

```rust
unsafe {
    NVIC::set_priority(Interrupt::DMA1_STREAM4, 0 << 4); // audio, highest
    NVIC::set_priority(Interrupt::TIM2,         1 << 4); // control
    NVIC::set_priority(Interrupt::TIM3,         2 << 4); // ui
}
```

## Shared state without `static mut`

`edition = "2024"` makes `&mut STATIC` a hard error (`static_mut_refs`). The `static mut
CTX: Option<T> = None` pattern in most embedded tutorials **will not compile**. Three
replacements, chosen by which boundary you are crossing.

### 1. Single-ISR ownership → `IsrCell`

For state touched by exactly one non-reentrant ISR (the audio engine, the SPI1 line
buffer). No locking needed, because there is no second accessor.

```rust
use core::cell::UnsafeCell;

/// A cell for state owned by exactly one interrupt handler.
///
/// SAFETY INVARIANT: the contained value is accessed only from the single ISR named
/// in the type's construction site, plus once from `main` before that interrupt is
/// enabled. Cortex-M interrupt handlers are non-reentrant, so this is exclusive.
pub struct IsrCell<T>(UnsafeCell<T>);

unsafe impl<T> Sync for IsrCell<T> {}

impl<T> IsrCell<T> {
    pub const fn new(v: T) -> Self { Self(UnsafeCell::new(v)) }

    /// # Safety
    /// Caller must be the owning ISR, or `main` prior to enabling that interrupt.
    pub unsafe fn get(&self) -> &mut T { unsafe { &mut *self.0.get() } }
}
```

This is the pattern for the audio path specifically, because it takes **zero critical
sections** — see below for why that matters.

### 2. Cross-priority parameter passing → seqlock

`TIM2` (prio 1) writes parameters; `DMA1_STREAM4` (prio 0) reads them. The audio ISR
must **never** take a critical section, because `interrupt::free` disables all
interrupts and directly injects jitter into the one path with a hard deadline.

Use a seqlock: two `ParamSnapshot` slots plus an `AtomicU32` sequence counter. The
writer bumps the sequence to odd, writes, bumps to even. The reader retries if the
sequence changed or is odd. Wait-free for the reader, which is exactly the property the
audio ISR needs. `ParamSnapshot` must be `Copy` and small.

### 3. Everything else → `critical_section::Mutex<RefCell<T>>`

For genuinely shared, non-time-critical state between `TIM2`, `TIM3`, and `main`. Safe,
checked, and the jitter cost is irrelevant at those priorities. You already have
`critical-section-single-core` enabled in `Cargo.toml`.

**Rule: no critical section, no allocation, no `defmt` formatting, and no panicking
index inside `DMA1_STREAM4`.** Bounds-check with masks (as the current wavetable code
already does with `& (TABLE_SIZE - 1)`) rather than relying on `panic!`.

## Audio data path

```
dsp::Engine ──fills──> [i16; BLOCK*4]  (one buffer, two halves, circular DMA)
                            │
                    DMA1 Stream 4 Ch 0
                            │
                        SPI2 / I2S ──> DAC
```

- **Circular DMA**, one buffer of `BLOCK * 2 channels * 2 halves` `i16`, interleaved L,R.
- Half-transfer interrupt → fill first half. Transfer-complete → fill second half.
- `BLOCK = 64` frames to start. 64 frames @ 44.1 kHz = 1.451 ms of latency and budget.
- DSP runs **directly in the DMA ISR**. Simplest, lowest jitter, fewest moving parts.
  If profiling later shows the fill is too long to sit at top priority, split it into a
  lower-priority task pended by the DMA ISR — but do not do this speculatively.

Verify `SPI2_TX = DMA1 Stream 4, Channel 0` against RM0383 Table 27 before wiring it.

## Control input path

- ADC1 in circular scan mode over N channels into `[u16; N]` via **DMA2 Stream 0 Ch 0**,
  hardware-triggered by TIM2. Zero CPU cost per conversion — this replaces the blocking
  `adc.convert()` calls currently sitting in the audio loop.
- `TIM2` at 1 kHz reads the DMA buffer, runs debounce state machines, applies a one-pole
  smoother to each pot, and publishes a `ParamSnapshot`.
- **Parameter smoothing is not optional.** The current code jumps `amplitude` in a single
  step every 256 samples, which is audible as zipper noise. One-pole toward the target at
  control rate fixes it.

## Display path

Sharp LS027B7DH01, 400x240, 1bpp. Framebuffer = 400*240/8 = **12,000 bytes**.

Three hardware quirks that will cost you a day each if you miss them:

1. **CS is active HIGH.** Opposite of every other SPI device.
2. **Data is LSB-first.** Either configure SPI for LSB-first or reverse bits in software.
3. **VCOM must be toggled** at 1–60 Hz or the panel degrades. Drive `EXTCOMIN` from a
   hardware PWM timer channel so it costs zero CPU and cannot be starved by a busy UI.

Max SCK is ~2 MHz. A full frame is ~12,482 bytes ≈ **50 ms**, so full-frame refresh at
30 Hz is physically impossible. Dirty-line tracking is a requirement, not an
optimization: keep a `[u8; 30]` bitmask (240 lines / 8) and DMA only changed lines. One
line is 52 bytes ≈ 208 µs.

Put display DMA on **DMA2** (SPI1_TX = DMA2 Stream 3 Ch 3) so it does not share a DMA
controller with audio on DMA1. Implement `embedded_graphics::DrawTarget` over the
framebuffer — good Rust trait practice and it gives you text, shapes, and fonts for free.

## When to migrate to RTIC

Bare interrupts are the right call now. Migrate when **two or more** of these are true:

- You have more than ~4 interrupt sources sharing state and the `IsrCell` safety
  comments have stopped being obviously true.
- You've hit a bug caused by a missing or mis-scoped critical section.
- You want software-pended tasks (deferred work at a chosen priority) and are about to
  hand-roll them.
- Adding a peripheral requires touching initialization in three places.

RTIC's priority-ceiling locking solves exactly these, and the concurrency table above
translates almost line-for-line into an `#[rtic::app]`. Migrating is a mechanical
half-day if `dsp` stayed HAL-free — which is the real reason for the crate split.
