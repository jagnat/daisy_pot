#![no_main]
#![no_std]

use cortex_m::asm;
use cortex_m_rt::entry;
use embassy_futures::yield_now;
use embassy_stm32::adc::{Adc, SampleTime};
use embassy_stm32::{self, Peri, adc, bind_interrupts, dma, gpio, pac, peripherals, sai, spi, time};
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
use crate::font::*;
// use crate::mushroom::MUSHROOM;

mod ls027b4dh01;
mod luts;
mod font;
mod font_garnet_9;
mod panic;
mod util;
// mod hobbit_hole;
// mod teapot;
mod mushroom;

const ADC_RANGE: i32 = u16::MAX as i32 + 1;
const ADC_MIDPT: u16 = 1 << 15;
const ANGLE_MAX: i32 = ADC_RANGE * 2;

// don't change this unless you change pll3 as well
const SAMPLE_RATE: Hertz = Hertz::khz(48);
const SAMPLES_PER_BLOCK: usize = 64;
const BLOCK_WORDS: usize = SAMPLES_PER_BLOCK * 2;
const ROOT_FREQ: f32 = 261.626; // C4

// equal temperament
// const PENTATONIC: [f32; 5] = [1.0, 1.122462, 1.259921, 1.498307, 1.681793];

// just intonation
const PENTATONIC: [f32; 5] = [1.0, 9.0 / 8.0, 5.0 / 4.0, 3.0 / 2.0, 5.0 / 3.0];
const PENTA_LEN: usize = PENTATONIC.len();
const OCTAVES_PER_CYCLE: i32 = 2;
const OCTAVE_MIN: i32 = -10;
const OCTAVE_MAX: i32 = 10;

#[derive(Copy, Clone)]
struct Osc {
    wave: Waveform,
    phase: u32,
    phase_inc: u32,
    amplitude: f32,
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
    DMA2_STREAM0 => dma::InterruptHandler<peripherals::DMA2_CH0>;
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

    let mut sharp_cs = gpio::Flex::new(p.PG10);
    sharp_cs.set_as_af_unchecked(5, gpio::AfType::output(gpio::OutputType::PushPull, gpio::Speed::High));
    let mut sharp_cfg = spi::Config::default();
    sharp_cfg.frequency = time::mhz(12);
    sharp_cfg.bit_order = spi::BitOrder::LsbFirst;
    let mut sharp_spi = spi::Spi::new_txonly(p.SPI1, p.PG11, p.PB5, p.DMA2_CH0, Irqs, sharp_cfg);

    // manual register hacks to turn on hardware cs (ss)
    pac::SPI1.cr1().modify(|w| w.set_spe(false));
    pac::SPI1.cfg2().modify(|w| w.set_ssm(false));
    pac::SPI1.cr1().modify(|w| w.set_spe(true));

    _spawner.spawn(audio_task(sai_tx).unwrap());
    _spawner.spawn(input_task(adc1, p.PC0, p.PA3, p.PB1, p.PB14).unwrap());
    _spawner.spawn(display_task(sharp_spi).unwrap());

    loop {
        core::future::pending::<()>().await;
    }
}

fn fill(buf: &mut [u32; BLOCK_WORDS], osc: &mut Osc) {
    for i in 0..(BLOCK_WORDS / 2) {
        let table = get_wave_table(osc.wave);
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
    let mut phase_inc: u32;
    let mut prev_ang: f32 = 0.0;
        
    let mut input = InputState {
        contpot: 0.0,
        volpot: 0.0,
        btn_pressed: false
    };

    let mut osc = Osc {
        wave: Waveform::Sine,
        phase: 0,
        phase_inc: 0,
        amplitude: 0.0,
    };

    let mut base_octave: i32 = 0;

    let phase_per_hz: f32 = (1u64 << 32) as f32 / (SAMPLE_RATE.0 as f32);
    let mut base_phase_inc = [0u32; PENTA_LEN];
    for i in 0..PENTA_LEN {
        base_phase_inc[i] = (ROOT_FREQ * PENTATONIC[i] * phase_per_hz) as u32;
    }
    let mut switch_osc;

    loop {
        switch_osc = false;
        if let Some(new_input) = INPUT_SIGNAL.try_take() {
            input = new_input;
            switch_osc = input.btn_pressed;
        }

        let ang = input.contpot;
        let delta = ang - prev_ang;
        if delta < -0.5 {
            base_octave = base_octave + OCTAVES_PER_CYCLE;
        } else if delta > 0.5 {
            base_octave = base_octave - OCTAVES_PER_CYCLE;
        }
        prev_ang = ang;

        let step = ang.clamp(0.0, 1.0) * (OCTAVES_PER_CYCLE * PENTA_LEN as i32) as f32;
        let degree = (step as usize) % PENTA_LEN;
        let octave = base_octave + (step as i32) / PENTA_LEN as i32;

        let base = base_phase_inc[degree];
        phase_inc = if octave >= 0 { base << octave } else { base >> (-octave) };
        if switch_osc {
            osc.wave = next_wave(osc.wave);
        }

        osc.phase_inc = phase_inc;
        osc.amplitude = input.volpot * i16::MAX as f32;

        fill(&mut test_tone, &mut osc);

        // todo: handle buffer underruns and display
        sai.write(&test_tone).await;
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

#[embassy_executor::task]
async fn display_task(mut sharp_spi: spi::Spi<'static, embassy_stm32::mode::Async, spi::mode::Master>) {
    let mut driver = SharpDisplayDriver::new();
    // driver.set_fullscreen(&MUSHROOM);
    while let Some(b) = driver.next_dirty_bytes() {
        sharp_spi.write(b).await.unwrap();
    }
    loop {
        driver.swap_vcom();
        let mut wrote = false;
        while let Some(b) = driver.next_dirty_bytes() {
            sharp_spi.write(b).await;
            wrote = true;
        }
        if !wrote {
            sharp_spi.write(&driver.vcom_cmd()).await;
        }
        Timer::after_millis(1000).await;
    }
}
