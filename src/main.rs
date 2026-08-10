#![no_main]
#![no_std]

use cortex_m::asm;
use cortex_m_rt::entry;
use embassy_stm32::adc::{Adc, SampleTime};
use embassy_stm32::{self, Peri, adc, bind_interrupts, dma, gpio, peripherals, sai};
use embassy_stm32::gpio::{AnyPin, Input, Level, Output, Pull, Speed};
use embassy_stm32::time::{Hertz};
use embassy_stm32::rcc;
use embassy_executor::{Spawner};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::{self, Duration, Ticker, Timer};
use embassy_sync::{signal::Signal};
use cortex_m::interrupt::Mutex;
use core::cell::RefCell;
use micromath::F32Ext;
// use pac::interrupt;
use crate::ls027b4dh01::SharpDisplayDriver;
use crate::luts::*;

mod ls027b4dh01;
mod luts;
mod font;
mod panic;
mod util;

const ADC_RANGE: i32 = u16::MAX as i32 + 1;
const ADC_MIDPT: u16 = 1 << 15;
const ANGLE_MAX: i32 = ADC_RANGE * 2;

// don't change this unless you change pll3 as well
const SAMPLE_RATE: Hertz = Hertz::khz(48);
const SAMPLES_PER_BLOCK: usize = 64;
const BLOCK_WORDS: usize = SAMPLES_PER_BLOCK * 2;
const ROOT_FREQ: f32 = 261.626; // C4

// equal temperament
const PENTATONIC: [f32; 5] = [1.0, 1.122462, 1.259921, 1.498307, 1.681793];

// just intonation
// const PENTATONIC: [f32; 5] = [1.0, 9.0 / 8.0, 5.0 / 4.0, 3.0 / 2.0, 5.0 / 3.0];
const PENTA_LEN: usize = PENTATONIC.len();
const OCTAVES_PER_CYCLE: i32 = 2;
const OCTAVE_MIN: i32 = -10;
const OCTAVE_MAX: i32 = 10;

#[derive(Copy, Clone)]
struct Osc {
    wave_idx: usize,
    phase: u32,
    phase_inc: u32,
    amplitude: f32
}

type SaiDriver = sai::Sai<'static, peripherals::SAI2, u32>;

// combine phase shifted tri waves from continuous pot
fn angle(val1: u16, val2: u16) -> i32 {
    if val1 < ADC_MIDPT {
        i32::from(val2)
    } else {
        ANGLE_MAX - 1 - i32::from(val2)
    }
}

fn db_vol_to_linear(val: f32 /* 0 - 1 */) -> f32 {
    if val == 0.0 {
        return 0.0;
    }
    let db = -60.0 + (val * 60.0);
    10.0f32.powf(db / 20.0)
}

bind_interrupts!(struct Irqs {
    DMA1_STREAM0 => dma::InterruptHandler<peripherals::DMA1_CH0>;
});

struct InputState {
    contpot: f32,
    volpot: f32,
    btn_pressed: bool,
}

static INPUT_SIGNAL: Signal<CriticalSectionRawMutex, InputState> = Signal::new();

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let mut cfg = embassy_stm32::Config::default();
    util::config_plls(&mut cfg);
    let p = embassy_stm32::init(cfg);
    panic::publish_sys_hz(rcc::clocks(&p.RCC).sys.to_hertz().unwrap().0);
    util::assert_pll(&p);

    let mut cp = cortex_m::Peripherals::take().unwrap();
    cp.SCB.enable_icache();

    let mut led = Output::new(p.PC7, Level::High, Speed::Medium);

    let (_, sai2_b) = sai::split_subblocks(p.SAI2);

    let mut sai2_tx_cfg = sai::Config::default();
    sai2_tx_cfg.clock_strobe = sai::ClockStrobe::Falling;
    sai2_tx_cfg.bit_order = sai::BitOrder::MsbFirst;
    sai2_tx_cfg.nodiv = true;
    // 24.576 mhz / 16 = 1.536mhz, which is 32 bits per 48khz
    sai2_tx_cfg.master_clock_divider = sai::MasterClockDivider::DIV16;

    let tx_buf = cortex_m::singleton!(: [u32; BLOCK_WORDS] = [0; BLOCK_WORDS] ).unwrap();

    // amp: SAI2 peripheral
    // amp din: pa0, ws/lrclk: pg9, bclk: pa2
    let sai_tx = sai::Sai::new_asynchronous(
        sai2_b,
        p.PA2,
        p.PA0,
        p.PG9,
        p.DMA1_CH0,
        tx_buf,
        Irqs,
        sai2_tx_cfg);

    let adc1 = adc::Adc::new_with_config(p.ADC1, adc::AdcConfig {
        resolution: Some(adc::Resolution::BITS16),
        averaging: Some(adc::Averaging::Samples4),
    });

    _spawner.spawn(audio_task(sai_tx).unwrap());
    _spawner.spawn(input_task(adc1, p.PC0, p.PA3, p.PB1, p.PB14).unwrap());

    loop {
        led.set_high();
        Timer::after_millis(1000).await;

        led.set_low();
        Timer::after_millis(1000).await;
    }
    
    // // sharp cs: pg10, sck: pg11, mosi: pb5
    // let sharp_freq: time::Hertz = 12.MHz();
    // let sharp_driver = cortex_m::singleton!(: SharpDisplayDriver = SharpDisplayDriver::new()).unwrap();
    // let sharp_sck = gpiog.pg11.into_alternate().speed(gpio::Speed::VeryHigh);
    // let sharp_mosi = gpiob.pb5.into_alternate().speed(gpio::Speed::VeryHigh);
    // let sharp_mode = spi::Mode {
    //     polarity: spi::Polarity::IdleLow,
    //     phase: spi::Phase::CaptureOnFirstTransition,
    // };
    // let sharp_hcs = gpiog.pg10.into_alternate();
    // let sharp_cfg = spi::Config::new(sharp_mode)
    //     .inter_word_delay(0.0)
    //     .hardware_cs(spi::HardwareCS {
    //         mode: spi::HardwareCSMode::FrameTransaction,
    //         assertion_delay: 0.000_003, // 3 micro-secs
    //         polarity: spi::Polarity::IdleLow,
    //     });
    // let sharp_spi: spi::Spi<_, _, u8> = dp.SPI1.spi((sharp_sck, spi::NoMiso, sharp_mosi, sharp_hcs), sharp_cfg, sharp_freq, ccdr.peripheral.SPI1, &ccdr.clocks);
    // // hack to set lsb-first since there isn't an easy api for it
    // let mut sharp_spi = sharp_spi.disable();
    // sharp_spi.inner_mut().cfg2.modify(|_, w| w.lsbfrst().lsbfirst());
    // let mut sharp_spi = sharp_spi.enable();
    //
    // // continuous pot pc0 / pa3
    // let mut pot1 = gpioc.pc0.into_analog();
    // let mut pot2 = gpioa.pa3.into_analog();
    // // vol pot pb1
    // let mut volpot = gpiob.pb1.into_analog();
    // // btn pb14
    // let button = gpiob.pb14.into_pull_up_input();
    // let mut delay = delay::Delay::new(cp.SYST, ccdr.clocks);
    //
    // let mut adc = adc::Adc::adc1(
    //     dp.ADC1,
    //     4.MHz(),
    //     &mut delay,
    //     ccdr.peripheral.ADC12,
    //     &ccdr.clocks,
    // )
    // .enable();
    // adc.set_resolution(adc::Resolution::TwelveBit);
    // adc.set_sample_time(adc::AdcSampleTime::T_64);
    //
    // let mut led = gpioc.pc7.into_push_pull_output();
    //
    // let mut vcom_tim = dp.TIM2.timer(1.Hz(), ccdr.peripheral.TIM2, &ccdr.clocks);
    //
    // let mut col: bool = false;
    //
    // let seed1: u32 = adc.read(&mut pot1).unwrap();
    // let seed2: u32 = adc.read(&mut pot2).unwrap();
    // let seed1 = seed1 as u16;
    // let seed2 = seed2 as u16;
    // let mut prev_mapped: i32 = angle(seed1, seed2);
    // let mut octave: i32 = 0;
    //
    // let phase_per_hz = (1u64 << 32) as f32 / SAMPLE_RATE.raw() as f32;
    // let mut base_inc = [0u32; PENTA_LEN];
    // for i in 0..PENTA_LEN {
    //     base_inc[i] = (ROOT_FREQ * PENTATONIC[i] * phase_per_hz) as u32;
    // }
    //
    // let mut phase_inc: u32 = 0;
    // let mut n: u32 = 0;
    //
    // let mut wave_idx: usize = 0;
    // let mut button_pressed = button.is_low();
    // let mut button_debounce: u32 = 0;
    //
    // let mut amplitude: f32;
    // loop {
    //     if vcom_tim.wait().is_ok() {
    //         for i in 0..400 {
    //             for j in 0..240 {
    //                 let add = (j / 24) % 2;
    //                 let set = ((i / 16) + add) % 2 == 0;
    //                 sharp_driver.set_pixel(i, j, set == col);
    //             }
    //         }
    //         col = !col;
    //         sharp_driver.swap_vcom();
    //         while let Some(b) = sharp_driver.next_dirty_bytes() {
    //             sharp_spi.write(b).unwrap();
    //         }
    //     }
    //     let raw_pressed = button.is_low();
    //     if raw_pressed != button_pressed {
    //         button_debounce += 1;
    //         if button_debounce >= DEBOUNCE_SAMPLES {
    //             button_pressed = raw_pressed;
    //             button_debounce = 0;
    //             if button_pressed {
    //                 wave_idx = (wave_idx + 1) % WAVES.len();
    //             }
    //         }
    //     } else {
    //         button_debounce = 0;
    //     }
    //
    //     n = n.wrapping_add(1);
    //     if n & 0xFF == 0 {
    //         let val1: u32 = adc.read(&mut pot1).unwrap();
    //         let val2: u32 = adc.read(&mut pot2).unwrap();
    //         let val3: u32 = adc.read(&mut volpot).unwrap();
    //         let val1 = val1 as u16;
    //         let val2 = val2 as u16;
    //         let val3 = val3 as u16;
    //         amplitude = pot_vol_to_linear(val3 as f32);
    //         amplitude *= 32767.0;
    //
    //         let mapped = angle(val1, val2);
    //
    //         // detec rotation wrap
    //         let delta = mapped - prev_mapped;
    //         if delta < -(ANGLE_MAX / 2) {
    //             octave = (octave + OCTAVES_PER_CYCLE).min(OCTAVE_MAX);
    //         } else if delta > ANGLE_MAX / 2 {
    //             octave = (octave - OCTAVES_PER_CYCLE).max(OCTAVE_MIN);
    //         }
    //         prev_mapped = mapped;
    //
    //         let step = mapped.clamp(0, ANGLE_MAX - 1) * (OCTAVES_PER_CYCLE * PENTA_LEN as i32) / ANGLE_MAX;
    //         let degree = (step % PENTA_LEN as i32) as usize;
    //         let eff = (octave + step / PENTA_LEN as i32).clamp(OCTAVE_MIN, OCTAVE_MAX);
    //         let base = base_inc[degree];
    //         phase_inc = if eff >= 0 { base << eff } else { base >> (-eff) };
    //         cortex_m::interrupt::free(|cs| {
    //             let mut osc = AUDIO.borrow(cs).borrow_mut();
    //             osc.wave_idx = wave_idx;
    //             osc.phase_inc = phase_inc;
    //             osc.amplitude = amplitude;
    //         });
    //     }
    // }
}

fn fill(buf: &mut [u32; BLOCK_WORDS], osc: &mut Osc) {
    for i in 0..(BLOCK_WORDS / 2) {
        let table = WAVES[osc.wave_idx];
        let idx = (osc.phase >> FRAC_BITS) as usize;
        let frac = (osc.phase & FRAC_MASK) as f32 / (1u32 << FRAC_BITS) as f32;
        let a = table[idx];
        let b = table[(idx + 1) & (TABLE_SIZE - 1)];
        let s = ((a + (b - a) * frac) * osc.amplitude) as i16;
        osc.phase = osc.phase.wrapping_add(osc.phase_inc);
        let word = s as u16 as u32;
        buf[i * 2 + 0] = word;
        buf[i * 2 + 1] = word;
    }
}

#[embassy_executor::task]
async fn audio_task(mut sai: SaiDriver) {
    let mut test_tone = [0u32; BLOCK_WORDS];
    let mut input = InputState {
        contpot: 0.0,
        volpot: 0.0,
        btn_pressed: false
    };

    loop {
        if let Some(new_input) = INPUT_SIGNAL.try_take() {
            input = new_input;
        }

        let peak = (2000.0 * input.volpot) as i16;
        for frame in 0..SAMPLES_PER_BLOCK {
            let sample = if frame < SAMPLES_PER_BLOCK / 2 {
                peak
            } else {
                -peak
            };
            let word = sample as u16 as u32;
            test_tone[frame * 2] = word;
            test_tone[frame * 2 + 1] = word;
        }

        sai.write(&test_tone).await.unwrap();
    }
}

#[embassy_executor::task]
async fn input_task(
    mut adc: Adc<'static, peripherals::ADC1>,
    mut pot1: Peri<'static, peripherals::PC0>,
    mut pot2: Peri<'static, peripherals::PA3>,
    mut volpot: Peri<'static, peripherals::PB1>,
    btn: Peri<'static, peripherals::PB14>) {
    let mut ticker = Ticker::every(Duration::from_millis(10));

    let mut control: f32;
    let mut vol: f32;
    let btn_read = Input::new(btn, Pull::Up);
    let mut last_btn_state = btn_read.is_low();

    loop {
        ticker.next().await;

        let re = adc.blocking_read(&mut pot1, SampleTime::CYCLES387_5);
        let im = adc.blocking_read(&mut pot2, SampleTime::CYCLES387_5);
        let ang = angle(re, im);
        control = ang as f32 / (ANGLE_MAX - 1) as f32;
        vol = adc.blocking_read(&mut volpot, SampleTime::CYCLES387_5) as f32 / (u16::MAX as f32);
        vol = db_vol_to_linear(vol);
        let btn_state = btn_read.is_low();

        INPUT_SIGNAL.signal(InputState {
            contpot: control,
            volpot: vol,
            btn_pressed: btn_state && last_btn_state != btn_state,
        });

        last_btn_state = btn_state;
    }
}

// #[interrupt]
// fn DMA1_STR0() {
//     static mut TRANSFER: Option<I2sDma> = None;
//     static mut LAST_FILLED: Option<dma::CurrentBuffer> = None;
//
//     let mut osc = cortex_m::interrupt::free(|cs| *AUDIO.borrow(cs).borrow());
//     let transfer = TRANSFER.get_or_insert_with(|| {
//         cortex_m::interrupt::free(|cs| {
//             AUDIO_TRANSFER
//                 .borrow(cs)
//                 .replace(None)
//                 .unwrap_or_else(|| panic!("DMA IRQ ran before its transfer was installed"))
//         })
//     });
//     let filled = unsafe {
//         transfer.next_transfer_with(|buf, current, remaining| {
//             let _ = remaining;
//             fill(buf, &mut osc);
//             (buf, current)
//         })
//     };
//
//     match filled {
//         Ok(half) => {
//             if *LAST_FILLED == Some(half) {
//                 panic!("DMA completed the same double buffer twice in a row");
//             }
//             *LAST_FILLED = Some(half);
//         }
//         Err(dma::DMAError::NotReady) => {
//             panic!("DMA IRQ without a completed transfer");
//         }
//         Err(dma::DMAError::SmallBuffer) => {
//             panic!("DMA replacement buffer length changed");
//         }
//         Err(dma::DMAError::Overflow) => {
//             panic!("DMA overran the audio ISR");
//         }
//     }
//
//     cortex_m::interrupt::free(|cs| AUDIO.borrow(cs).borrow_mut().phase = osc.phase);
// }
