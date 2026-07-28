#![no_main]
#![no_std]

use defmt_rtt as _;
use panic_probe as _;

#[defmt::panic_handler]
fn panic() -> ! {
    cortex_m::asm::udf()
}

#[cortex_m_rt::exception]
unsafe fn HardFault(_frame: &cortex_m_rt::ExceptionFrame) -> ! {
    semihosting::process::exit(1);
}

use cortex_m::interrupt::Mutex;
use core::cell::RefCell;
use micromath::{F32Ext};

use stm32f4xx_hal::prelude::*;
use stm32f4xx_hal::i2s::I2s;
use stm32f4xx_hal::i2s::stm32_i2s_v12x::transfer::{Transmit, Master, Philips};
use stm32f4xx_hal::i2s::stm32_i2s_v12x::driver::{I2sDriverConfig, I2sDriver, DataFormat::Data16Channel16};
use stm32f4xx_hal::interrupt::{DMA1_STREAM4};
use stm32f4xx_hal::pac::{Peripherals, interrupt, DMA1, SPI2, NVIC};
use stm32f4xx_hal::rcc::Config;
 use stm32f4xx_hal::adc::{Adc};
use stm32f4xx_hal::adc::config::AdcConfig;
use stm32f4xx_hal::dma::{StreamsTuple, Transfer, Stream4, MemoryToPeripheral};
use stm32f4xx_hal::dma::config::{DmaConfig, Priority};

const SAMPLE_RATE: u32 = 44100;

const TABLE_BITS: u32 = 8;
const TABLE_SIZE: usize = 1 << TABLE_BITS;
const FRAC_BITS: u32 = 32 - TABLE_BITS;
const FRAC_MASK: u32 = (1 << FRAC_BITS) - 1;

const WAVES: [&[f32; TABLE_SIZE]; 3] = [&SINE, &SQUARE, &SAW];

const DEBOUNCE_SAMPLES: u32 = 441;

const ROOT_FREQ: f32 = 261.626; // C4

const PENTATONIC: [f32; 5] = [1.0, 1.122462, 1.259921, 1.498307, 1.681793];
const PENTA_LEN: usize = PENTATONIC.len();
const ANGLE_MAX: i32 = 8192;
const OCTAVES_PER_CYCLE: i32 = 2;
const OCTAVE_MIN: i32 = -3;
const OCTAVE_MAX: i32 = 3;

#[derive(Copy, Clone)]
struct Osc {wave_idx: usize, phase: u32, phase_inc: u32, amplitude: f32 }

static AUDIO: Mutex<RefCell<Osc>> = Mutex::new(RefCell::new(Osc {
    wave_idx: 0,
    phase: 0,
    phase_inc: 42_852_281, // A440
    amplitude: 0.0,
}));

// combine phase shifted triangle waves into one
fn angle(val1: u16, val2: u16) -> i32 {
    (if val1 < 2048 {
        val2 as i32 - 4095
    } else {
        4096 - val2 as i32
    }) + 4096
}

// interpret pot as db and map to linear amplitude
fn pot_vol_to_linear(val: f32) -> f32 {
    if val == 0.0 {
        return 0.0
    }
    let norm = val / 4096.0;
    let db = -60.0 + (norm * 60.0);
    10.0f32.powf(db / 20.0)
}

const SINE: [f32; TABLE_SIZE] = [
    0.0, 0.02454123, 0.04906767, 0.07356456, 0.09801714, 0.1224107, 0.1467305, 0.1709619,
    0.1950903, 0.2191012, 0.2429802, 0.2667128, 0.2902847, 0.3136817, 0.3368899, 0.359895,
    0.3826834, 0.4052413, 0.4275551, 0.4496113, 0.4713967, 0.4928982, 0.5141027, 0.5349976,
    0.5555702, 0.5758082, 0.5956993, 0.6152316, 0.6343933, 0.6531728, 0.671559, 0.6895405,
    0.7071068, 0.7242471, 0.7409511, 0.7572088, 0.7730105, 0.7883464, 0.8032075, 0.8175848,
    0.8314696, 0.8448536, 0.8577286, 0.870087, 0.8819213, 0.8932243, 0.9039893, 0.9142098,
    0.9238795, 0.9329928, 0.9415441, 0.9495282, 0.9569403, 0.9637761, 0.9700313, 0.9757021,
    0.9807853, 0.9852776, 0.9891765, 0.9924795, 0.9951847, 0.9972905, 0.9987955, 0.9996988,
    1.0, 0.9996988, 0.9987955, 0.9972905, 0.9951847, 0.9924795, 0.9891765, 0.9852776,
    0.9807853, 0.9757021, 0.9700313, 0.9637761, 0.9569403, 0.9495282, 0.9415441, 0.9329928,
    0.9238795, 0.9142098, 0.9039893, 0.8932243, 0.8819213, 0.870087, 0.8577286, 0.8448536,
    0.8314696, 0.8175848, 0.8032075, 0.7883464, 0.7730105, 0.7572088, 0.7409511, 0.7242471,
    0.7071068, 0.6895405, 0.671559, 0.6531728, 0.6343933, 0.6152316, 0.5956993, 0.5758082,
    0.5555702, 0.5349976, 0.5141027, 0.4928982, 0.4713967, 0.4496113, 0.4275551, 0.4052413,
    0.3826834, 0.359895, 0.3368899, 0.3136817, 0.2902847, 0.2667128, 0.2429802, 0.2191012,
    0.1950903, 0.1709619, 0.1467305, 0.1224107, 0.09801714, 0.07356456, 0.04906767, 0.02454123,
    0.0, -0.02454123, -0.04906767, -0.07356456, -0.09801714, -0.1224107, -0.1467305, -0.1709619,
    -0.1950903, -0.2191012, -0.2429802, -0.2667128, -0.2902847, -0.3136817, -0.3368899, -0.359895,
    -0.3826834, -0.4052413, -0.4275551, -0.4496113, -0.4713967, -0.4928982, -0.5141027, -0.5349976,
    -0.5555702, -0.5758082, -0.5956993, -0.6152316, -0.6343933, -0.6531728, -0.671559, -0.6895405,
    -0.7071068, -0.7242471, -0.7409511, -0.7572088, -0.7730105, -0.7883464, -0.8032075, -0.8175848,
    -0.8314696, -0.8448536, -0.8577286, -0.870087, -0.8819213, -0.8932243, -0.9039893, -0.9142098,
    -0.9238795, -0.9329928, -0.9415441, -0.9495282, -0.9569403, -0.9637761, -0.9700313, -0.9757021,
    -0.9807853, -0.9852776, -0.9891765, -0.9924795, -0.9951847, -0.9972905, -0.9987955, -0.9996988,
    -1.0, -0.9996988, -0.9987955, -0.9972905, -0.9951847, -0.9924795, -0.9891765, -0.9852776,
    -0.9807853, -0.9757021, -0.9700313, -0.9637761, -0.9569403, -0.9495282, -0.9415441, -0.9329928,
    -0.9238795, -0.9142098, -0.9039893, -0.8932243, -0.8819213, -0.870087, -0.8577286, -0.8448536,
    -0.8314696, -0.8175848, -0.8032075, -0.7883464, -0.7730105, -0.7572088, -0.7409511, -0.7242471,
    -0.7071068, -0.6895405, -0.671559, -0.6531728, -0.6343933, -0.6152316, -0.5956993, -0.5758082,
    -0.5555702, -0.5349976, -0.5141027, -0.4928982, -0.4713967, -0.4496113, -0.4275551, -0.4052413,
    -0.3826834, -0.359895, -0.3368899, -0.3136817, -0.2902847, -0.2667128, -0.2429802, -0.2191012,
    -0.1950903, -0.1709619, -0.1467305, -0.1224107, -0.09801714, -0.07356456, -0.04906767, -0.02454123,
];

const SQUARE: [f32; TABLE_SIZE] = [
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0,
    -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0,
    -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0,
    -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0,
    -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0,
    -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0,
    -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0,
    -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0,
    -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0,
    -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0,
    -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0,
    -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0,
    -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0,
    -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0,
    -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0,
    -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0,
];

const SAW: [f32; TABLE_SIZE] = [
    -1.0, -0.9921875, -0.984375, -0.9765625, -0.96875, -0.9609375, -0.953125, -0.9453125,
    -0.9375, -0.9296875, -0.921875, -0.9140625, -0.90625, -0.8984375, -0.890625, -0.8828125,
    -0.875, -0.8671875, -0.859375, -0.8515625, -0.84375, -0.8359375, -0.828125, -0.8203125,
    -0.8125, -0.8046875, -0.796875, -0.7890625, -0.78125, -0.7734375, -0.765625, -0.7578125,
    -0.75, -0.7421875, -0.734375, -0.7265625, -0.71875, -0.7109375, -0.703125, -0.6953125,
    -0.6875, -0.6796875, -0.671875, -0.6640625, -0.65625, -0.6484375, -0.640625, -0.6328125,
    -0.625, -0.6171875, -0.609375, -0.6015625, -0.59375, -0.5859375, -0.578125, -0.5703125,
    -0.5625, -0.5546875, -0.546875, -0.5390625, -0.53125, -0.5234375, -0.515625, -0.5078125,
    -0.5, -0.4921875, -0.484375, -0.4765625, -0.46875, -0.4609375, -0.453125, -0.4453125,
    -0.4375, -0.4296875, -0.421875, -0.4140625, -0.40625, -0.3984375, -0.390625, -0.3828125,
    -0.375, -0.3671875, -0.359375, -0.3515625, -0.34375, -0.3359375, -0.328125, -0.3203125,
    -0.3125, -0.3046875, -0.296875, -0.2890625, -0.28125, -0.2734375, -0.265625, -0.2578125,
    -0.25, -0.2421875, -0.234375, -0.2265625, -0.21875, -0.2109375, -0.203125, -0.1953125,
    -0.1875, -0.1796875, -0.171875, -0.1640625, -0.15625, -0.1484375, -0.140625, -0.1328125,
    -0.125, -0.1171875, -0.109375, -0.1015625, -0.09375, -0.0859375, -0.078125, -0.0703125,
    -0.0625, -0.0546875, -0.046875, -0.0390625, -0.03125, -0.0234375, -0.015625, -0.0078125,
    0.0, 0.0078125, 0.015625, 0.0234375, 0.03125, 0.0390625, 0.046875, 0.0546875,
    0.0625, 0.0703125, 0.078125, 0.0859375, 0.09375, 0.1015625, 0.109375, 0.1171875,
    0.125, 0.1328125, 0.140625, 0.1484375, 0.15625, 0.1640625, 0.171875, 0.1796875,
    0.1875, 0.1953125, 0.203125, 0.2109375, 0.21875, 0.2265625, 0.234375, 0.2421875,
    0.25, 0.2578125, 0.265625, 0.2734375, 0.28125, 0.2890625, 0.296875, 0.3046875,
    0.3125, 0.3203125, 0.328125, 0.3359375, 0.34375, 0.3515625, 0.359375, 0.3671875,
    0.375, 0.3828125, 0.390625, 0.3984375, 0.40625, 0.4140625, 0.421875, 0.4296875,
    0.4375, 0.4453125, 0.453125, 0.4609375, 0.46875, 0.4765625, 0.484375, 0.4921875,
    0.5, 0.5078125, 0.515625, 0.5234375, 0.53125, 0.5390625, 0.546875, 0.5546875,
    0.5625, 0.5703125, 0.578125, 0.5859375, 0.59375, 0.6015625, 0.609375, 0.6171875,
    0.625, 0.6328125, 0.640625, 0.6484375, 0.65625, 0.6640625, 0.671875, 0.6796875,
    0.6875, 0.6953125, 0.703125, 0.7109375, 0.71875, 0.7265625, 0.734375, 0.7421875,
    0.75, 0.7578125, 0.765625, 0.7734375, 0.78125, 0.7890625, 0.796875, 0.8046875,
    0.8125, 0.8203125, 0.828125, 0.8359375, 0.84375, 0.8515625, 0.859375, 0.8671875,
    0.875, 0.8828125, 0.890625, 0.8984375, 0.90625, 0.9140625, 0.921875, 0.9296875,
    0.9375, 0.9453125, 0.953125, 0.9609375, 0.96875, 0.9765625, 0.984375, 0.9921875,
];

const SAMPLES_PER_BLOCK: usize = 64;
const BLOCK_WORDS: usize = SAMPLES_PER_BLOCK * 2;
type I2sDma = Transfer<Stream4<DMA1>, 0, I2sDriver<I2s<SPI2>, Master, Transmit, Philips>, MemoryToPeripheral, &'static mut [u16; BLOCK_WORDS]>;
static AUDIO_TRANSFER: Mutex<RefCell<Option<I2sDma>>> = Mutex::new(RefCell::new(None));

#[cortex_m_rt::entry]
fn main() -> ! {
    defmt::println!("i2s_write_test: starting");

    // Audio out dma bufs
    let audio_buf_1 = cortex_m::singleton!(: [u16; BLOCK_WORDS] = [0; BLOCK_WORDS]).unwrap();
    let audio_buf_2 = cortex_m::singleton!(: [u16; BLOCK_WORDS] = [0; BLOCK_WORDS]).unwrap();
    for b in audio_buf_1.iter_mut() {
        *b = 0;
    }

    let mut cp = cortex_m::Peripherals::take().unwrap();
    let dp = Peripherals::take().unwrap();

    let mut rcc = dp.RCC.freeze(
        Config::hse(25u32.MHz())
            .sysclk(96.MHz())
            .i2s_clk(150.MHz()),
    );

    let gpioa = dp.GPIOA.split(&mut rcc);
    let gpiob = dp.GPIOB.split(&mut rcc);

    // sin/cos comps of continuous
    let pot1 = gpioa.pa0.into_analog();
    let pot2 = gpioa.pa1.into_analog();

    // waveform select
    let button = gpiob.pb14.into_pull_up_input();

    let volpot = gpioa.pa2.into_analog();

    let mut adc = Adc::new(dp.ADC1, true, AdcConfig::default(), &mut rcc);
    let i2s_pins = (gpiob.pb12, gpiob.pb10, stm32f4xx_hal::pac::SPI2::NoMck, gpiob.pb15);
    let i2s = I2s::new(dp.SPI2, i2s_pins, &mut rcc);
    let i2s_driver_config = I2sDriverConfig::new_master().transmit().standard(Philips).data_format(Data16Channel16).request_frequency(SAMPLE_RATE);
    let mut i2s_driver = I2sDriver::new(i2s, i2s_driver_config);
    i2s_driver.set_tx_dma(true);
    let tx_stream = StreamsTuple::new(dp.DMA1, &mut rcc).4;
    let i2s_config = DmaConfig::default().memory_increment(true).double_buffer(true).transfer_complete_interrupt(true).priority(Priority::VeryHigh);
    let mut tx_transfer = Transfer::init_memory_to_peripheral(tx_stream, i2s_driver, audio_buf_1, Some(audio_buf_2), i2s_config);
    tx_transfer.start(|i2s| i2s.enable());

    cortex_m::interrupt::free(|cs| *AUDIO_TRANSFER.borrow(cs).borrow_mut() = Some(tx_transfer));

    unsafe {
        cp.NVIC.set_priority(DMA1_STREAM4, 0 << 4);
        NVIC::unmask(DMA1_STREAM4);
    }

    // let sharp_pins = (
    //     Some(gpiob.pb3.into_alternate().speed(Speed::VeryHigh).internal_pull_up(true)),// sck
    //     SPI3::NoMiso, // miso
    //     Some(gpiob.pb5.into_alternate().speed(Speed::VeryHigh))); // mosi
    // let sharp_mode = Mode {
    //     polarity: Polarity::IdleLow,
    //     phase: Phase::CaptureOnFirstTransition,
    // };
    // let mut sharp = Spi::new(dp.SPI3, sharp_pins, sharp_mode, 300.Hz(), &mut rcc);

    // phase increment
    let phase_per_hz = (1u64 << 32) as f32 / SAMPLE_RATE as f32;
    let mut base_inc = [0u32; PENTA_LEN];
    for i in 0..PENTA_LEN {
        base_inc[i] = (ROOT_FREQ * PENTATONIC[i] * phase_per_hz) as u32;
    }

    let seed1 = adc.convert(&pot1, stm32f4xx_hal::adc::config::SampleTime::Cycles_28);
    let seed2 = adc.convert(&pot2, stm32f4xx_hal::adc::config::SampleTime::Cycles_28);
    let mut prev_mapped: i32 = angle(seed1, seed2);
    let mut octave: i32 = 0;

    // let mut phase: u32 = 0;
    let step0 = prev_mapped.clamp(0, ANGLE_MAX - 1) * (OCTAVES_PER_CYCLE * PENTA_LEN as i32) / ANGLE_MAX;
    let mut phase_inc: u32 = base_inc[(step0 % PENTA_LEN as i32) as usize] << (step0 / PENTA_LEN as i32);
    let mut n: u32 = 0;

    let mut wave_idx: usize = 0;
    let mut button_pressed = button.is_low();
    let mut button_debounce: u32 = 0;

    let mut amplitude: f32 = 0.0;

    loop {

        // debounce
        let raw_pressed = button.is_low();
        if raw_pressed != button_pressed {
            button_debounce += 1;
            if button_debounce >= DEBOUNCE_SAMPLES {
                button_pressed = raw_pressed;
                button_debounce = 0;
                if button_pressed {
                    wave_idx = (wave_idx + 1) % WAVES.len();
                    defmt::println!("waveform -> {}", wave_idx);
                }
            }
        } else {
            button_debounce = 0;
        }

        n = n.wrapping_add(1);
        if n & 0xFF == 0 {
            let val1: u16 = adc.convert(&pot1, stm32f4xx_hal::adc::config::SampleTime::Cycles_28);
            let val2: u16 = adc.convert(&pot2, stm32f4xx_hal::adc::config::SampleTime::Cycles_28);
            let val3: u16 = adc.convert(&volpot, stm32f4xx_hal::adc::config::SampleTime::Cycles_28);
            amplitude = pot_vol_to_linear(val3 as f32);
            amplitude *= 32767.0;

            let mapped = angle(val1, val2);

            // detec rotation wrap
            let delta = mapped - prev_mapped;
            if delta < -(ANGLE_MAX / 2) {
                octave = (octave + OCTAVES_PER_CYCLE).min(OCTAVE_MAX);
            } else if delta > ANGLE_MAX / 2 {
                octave = (octave - OCTAVES_PER_CYCLE).max(OCTAVE_MIN);
            }
            prev_mapped = mapped;

            let step = mapped.clamp(0, ANGLE_MAX - 1) * (OCTAVES_PER_CYCLE * PENTA_LEN as i32) / ANGLE_MAX;
            let degree = (step % PENTA_LEN as i32) as usize;
            let eff = (octave + step / PENTA_LEN as i32).clamp(OCTAVE_MIN, OCTAVE_MAX);
            let base = base_inc[degree];
            phase_inc = if eff >= 0 { base << eff } else { base >> (-eff) };
        }

        cortex_m::interrupt::free(|cs| {
            let mut osc = AUDIO.borrow(cs).borrow_mut();
            osc.wave_idx = wave_idx;
            osc.phase_inc = phase_inc;
            osc.amplitude = amplitude;
        });
    }
}

fn fill(buf: &mut [u16; BLOCK_WORDS], osc: &mut Osc) {
    for i in 0..64 {
        let table = WAVES[osc.wave_idx];
        let idx = (osc.phase >> FRAC_BITS) as usize;
        let frac = (osc.phase & FRAC_MASK) as f32 / (1u32 << FRAC_BITS) as f32;
        let a = table[idx];
        let b = table[(idx + 1) & (TABLE_SIZE - 1)];
        let s = ((a + (b - a) * frac) * osc.amplitude) as i16;
        osc.phase = osc.phase.wrapping_add(osc.phase_inc);
        buf[i * 2 + 0] = s as u16;
        buf[i * 2 + 1] = s as u16;
    }
}

#[interrupt]
fn DMA1_STREAM4() {
    static mut TRANSFER: Option<I2sDma> = None;
    let mut osc = cortex_m::interrupt::free(|cs| *AUDIO.borrow(cs).borrow());
    let transfer = TRANSFER.get_or_insert_with(|| {
        cortex_m::interrupt::free(|cs| AUDIO_TRANSFER.borrow(cs).replace(None).unwrap())
    });
    unsafe {
        let _ = transfer.next_transfer_with(|buf, _| {
            fill(buf, &mut osc);
            (buf, ())
        });
    }
    cortex_m::interrupt::free(|cs| AUDIO.borrow(cs).borrow_mut().phase = osc.phase);
}

