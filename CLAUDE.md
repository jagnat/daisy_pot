# daisy_pot

An audio sequencer/synth being built on a Daisy Seed3. This is exploratory
instrument work, not a fixed spec.

## Working style

- The user writes the code. Explain APIs, trade-offs, and where to read source;
  only write code when explicitly asked.
- Let structure grow from a real problem. Keep the crate flat: new driver means
  `src/foo.rs` and one `mod foo;` in `main.rs`.
- Do not create docs or design files unless asked. Keep lasting facts as nearby
  code comments.
- Keep hardware pointers short and practical.

## Hardware

- Daisy Seed3: STM32H750IBK6 Cortex-M7 at 480 MHz. RM0433.
- 128K internal flash; 1M SRAM; 64M SDRAM; 8M QSPI.
- TAC5242 codec: hardware-strapped, internal SAI1 bus; analog output on Seed3
  physical pins 18/19.
- TPA6132A2 headphone amp: analog input from the Seed3 codec outputs. No I2C;
  enable and gain are hardware controlled.
- Sharp LS027B7DH01 memory LCD: 400x240, 1 bpp.
- Flash with DFU: hold BOOT, tap RESET, then `cargo run`. There is no probe,
  RTT, or defmt; the LED is the current diagnostic output.

| Function | Pin | Header | Peripheral |
|---|---|---:|---|
| LED | PC7 | — | GPIO |
| Pot 1 | PC0 | 22 / A0 | ADC1_INP10 |
| Pot 2 | PA3 | 23 / A1 | ADC1_INP15 |
| Volume | PB1 | 24 / A2 | ADC1_INP5 |
| Audio out L | — | 18 | TAC5242 analog out → TPA left in |
| Audio out R | — | 19 | TAC5242 analog out → TPA right in |
| Audio AGND | — | 20 | TPA ground |
| Audio 3V3A | — | 21 | TPA supply |
| Display CS | PG10 | 8 / D7 | GPIO, active high |
| Display SCK | PG11 | 9 / D8 | SPI1_SCK, AF5 |
| Display MOSI | PB5 | 11 / D10 | SPI1_MOSI, AF5 |
| Button | PB14 | 36 / D29 | GPIO, pull-up |

SAI2 and its header pins 31–35 are currently free. The onboard codec bus uses
SAI1 on internal Seed3 connections and consumes no header GPIO.

Do not allocate these pins: D1–D6 are SDMMC1; D11/D12 are I2C1; D13/D14 are
USART1 MIDI. PB14/PB15 are USB_HS, so the button gives up that second USB
port. DFU remains on the onboard USB-C (PA11/PA12).

## Electrical notes

- Tie AGND and DGND together once, near the module. They are not tied on the
  Seed3.
- Run pots from 3V3A/AGND and the display from 3V3D/DGND.
- Power the TPA6132A2 from 3V3A and AGND. Its breakout pulls CTRL high by
  default; DET and CTRL can remain disconnected unless firmware needs them.

## Rules that matter

- Audio is the hard deadline. Do not let UI or control work delay the audio
  DMA ISR.
- Keep DSP independent of HAL types: functions should work on numbers and
  slices, not DMA transfers.
- DMA1/DMA2 cannot access DTCM or ITCM. Audio buffers belong in AXI SRAM (the
  `RAM` region in `memory.x`) or another DMA-visible SRAM bank.
- D-cache needs explicit handling or a non-cacheable MPU region for DMA
  buffers if it is enabled.
- The PAC/HAL does not host-test on macOS. Host DSP tests would need a separate
  crate if that becomes worthwhile.
- Keep this pin table up to date.
