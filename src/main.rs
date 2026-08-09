#![no_main]
#![no_std]

use cortex_m::asm;
use cortex_m_rt::entry;
use stm32h7xx_hal::gpio;
use stm32h7xx_hal::interrupt::DMA1_STR0;
use stm32h7xx_hal::pac::NVIC;
use stm32h7xx_hal::delay;
use stm32h7xx_hal::rcc;
use stm32h7xx_hal::adc;
use stm32h7xx_hal::rcc::rec::Sai23ClkSelGetter;
use stm32h7xx_hal::sai;
use stm32h7xx_hal::spi;
use stm32h7xx_hal::time;
use stm32h7xx_hal::dma;
use stm32h7xx_hal::time::Hertz;
use stm32h7xx_hal::{pac, prelude::*, rcc::PllConfigStrategy};
use cortex_m::interrupt::Mutex;
use core::cell::RefCell;
use micromath::F32Ext;
use pac::interrupt;
use crate::ls027b4dh01::SharpDisplayDriver;
use crate::luts::*;

mod ls027b4dh01;
mod luts;
mod font;
mod panic;

const SAMPLE_RATE: time::Hertz = Hertz::from_raw(48000);
// something something set to 256 * sample rate minimum
const PLL3_P_HZ: time::Hertz = time::Hertz::from_raw(SAMPLE_RATE.raw() * 257);
const SAMPLES_PER_BLOCK: usize = 64;
const BLOCK_WORDS: usize = SAMPLES_PER_BLOCK * 2;
const DEBOUNCE_SAMPLES: u32 = 441;
const ROOT_FREQ: f32 = 261.626; // C4

// equal temperament
// const PENTATONIC: [f32; 5] = [1.0, 1.122462, 1.259921, 1.498307, 1.681793];

// just intonation
const PENTATONIC: [f32; 5] = [1.0, 9.0 / 8.0, 5.0 / 4.0, 3.0 / 2.0, 5.0 / 3.0];
const PENTA_LEN: usize = PENTATONIC.len();
const ANGLE_MAX: i32 = 8192;
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

static AUDIO: Mutex<RefCell<Osc>> = Mutex::new(RefCell::new(Osc {
    wave_idx: 0,
    phase: 0,
    phase_inc: 42_852_281, // A440
    amplitude: 0.0,
}));

// combine phase shifted tri waves from continuous pot
fn angle(val1: u16, val2: u16) -> i32 {
    (if val1 < 2048 {
        val2 as i32 - 4095
    } else {
        4096 - val2 as i32
    }) + 4096
}

fn pot_vol_to_linear(val: f32) -> f32 {
    if val == 0.0 {
        return 0.0;
    }
    let norm = val / 4096.0;
    let db = -60.0 + (norm * 60.0);
    10.0f32.powf(db / 20.0)
}

type I2sDma = dma::Transfer<
    dma::dma::Stream0<pac::DMA1>,
    sai::dma::ChannelB<pac::SAI2>,
    dma::MemoryToPeripheral,
    &'static mut [u32; BLOCK_WORDS],
    dma::DBTransfer
>;
static AUDIO_TRANSFER: Mutex<RefCell<Option<I2sDma>>> = Mutex::new(RefCell::new(None));

#[entry]
fn main() -> ! {
    let audio_buf_1 = cortex_m::singleton!(: [u32; BLOCK_WORDS] = [0; BLOCK_WORDS]).unwrap();
    let audio_buf_2 = cortex_m::singleton!(: [u32; BLOCK_WORDS] = [0; BLOCK_WORDS]).unwrap();
    for b in audio_buf_1.iter_mut() {
        *b = 0;
    }

    let mut cp = cortex_m::Peripherals::take().unwrap();
    let dp = pac::Peripherals::take().unwrap();

    let pwr = dp.PWR.constrain().vos0(&dp.SYSCFG);
    let pwrcfg = pwr.freeze();

    let mut rcc = dp.RCC.constrain();
    let mut ccdr = rcc
        .use_hse(16.MHz())
        .pll1_strategy(PllConfigStrategy::Iterative)
        .pll1_q_ck(96.MHz())
        .pll3_strategy(PllConfigStrategy::Iterative)
        .pll3_p_ck(PLL3_P_HZ)
        .sys_ck(480.MHz())
        .freeze(pwrcfg, &dp.SYSCFG);


    panic::publish_sys_hz(ccdr.clocks.sys_ck().raw());
    assert!(ccdr.clocks.sys_ck().raw() == 480_000_000);
    ccdr.peripheral.kernel_adc_clk_mux(rcc::rec::AdcClkSel::Per);
    let sai2_rec = ccdr.peripheral.kernel_sai23_clk_mux(rcc::rec::Sai23ClkSel::Pll3P);

    let gpioa = dp.GPIOA.split(ccdr.peripheral.GPIOA);
    let gpiob = dp.GPIOB.split(ccdr.peripheral.GPIOB);
    let gpioc = dp.GPIOC.split(ccdr.peripheral.GPIOC);
    let gpiog = dp.GPIOG.split(ccdr.peripheral.GPIOG);

    // sharp cs: pg10, sck: pg11, mosi: pb5
    let sharp_freq: time::Hertz = 12.MHz();
    let sharp_driver = cortex_m::singleton!(: SharpDisplayDriver = SharpDisplayDriver::new()).unwrap();
    let sharp_sck = gpiog.pg11.into_alternate().speed(gpio::Speed::VeryHigh);
    let sharp_mosi = gpiob.pb5.into_alternate().speed(gpio::Speed::VeryHigh);
    let sharp_mode = spi::Mode {
        polarity: spi::Polarity::IdleLow,
        phase: spi::Phase::CaptureOnFirstTransition,
    };
    let sharp_hcs = gpiog.pg10.into_alternate();
    let sharp_cfg = spi::Config::new(sharp_mode)
        .inter_word_delay(0.0)
        .hardware_cs(spi::HardwareCS {
            mode: spi::HardwareCSMode::FrameTransaction,
            assertion_delay: 0.000_003, // 3 micro-secs
            polarity: spi::Polarity::IdleLow,
        });
    let sharp_spi: spi::Spi<_, _, u8> = dp.SPI1.spi((sharp_sck, spi::NoMiso, sharp_mosi, sharp_hcs), sharp_cfg, sharp_freq, ccdr.peripheral.SPI1, &ccdr.clocks);
    // hack to set lsb-first since there isn't an easy api for it
    let mut sharp_spi = sharp_spi.disable();
    sharp_spi.inner_mut().cfg2.modify(|_, w| w.lsbfrst().lsbfirst());
    let mut sharp_spi = sharp_spi.enable();


    // amp: SAI2 peripheral
    // amp din: pa0, ws/lrclk: pg9, bclk: pa2
    let i2s2_pins = (
        gpioa.pa1.into_alternate(), // mclk not used
        gpioa.pa2.into_alternate(), // sck
        gpiog.pg9.into_alternate(), // ws
        gpioa.pa0.into_alternate(), // data in
        None::<gpio::PD11<gpio::AF10>> // data in 2 (not used)
    );
    let i2s2_tx_config = sai::I2SChanConfig::new(sai::I2SDir::Tx)
        .set_clock_strobe(sai::I2SClockStrobe::Falling)
        .set_protocol(sai::I2SProtocol::MSB)
        .set_frame_sync_before(true);
    let mut sai2 = dp.SAI2.i2s_ch_b(
        i2s2_pins,
        SAMPLE_RATE,
        sai::I2SDataSize::BITS_16,
        ccdr.peripheral.SAI2,
        &ccdr.clocks,
        sai::I2sUsers::new(i2s2_tx_config));

    let dma1_stream0 = dma::dma::StreamsTuple::new(dp.DMA1, ccdr.peripheral.DMA1).0;
    let dma_cfg = dma::dma::DmaConfig::default()
        .priority(dma::config::Priority::High)
        .memory_increment(true)
        .peripheral_increment(false)
        .double_buffer(true)
        .transfer_complete_interrupt(true)
        .fifo_enable(false);
    let mut the_dma: dma::Transfer<_, _, dma::MemoryToPeripheral, _, _> = dma::Transfer::init(
        dma1_stream0,
        unsafe { pac::Peripherals::steal().SAI2.dma_ch_b() },
        audio_buf_1,
        Some(audio_buf_2),
        dma_cfg);
    cortex_m::interrupt::free(|cs| {
        let mut osc = AUDIO.borrow(cs).borrow_mut();
        osc.amplitude = 8000.0;
    });

    the_dma.start(|f| {
    });

    cortex_m::interrupt::free(|cs| *AUDIO_TRANSFER.borrow(cs).borrow_mut() = Some(the_dma));

    sai2.enable_dma(sai::SaiChannel::ChannelB);
    sai2.enable();

    unsafe {
        cp.NVIC.set_priority(DMA1_STR0, 0<<4);
        NVIC::unmask(DMA1_STR0);
    }

    // continuous pot pc0 / pa3
    let mut pot1 = gpioc.pc0.into_analog();
    let mut pot2 = gpioa.pa3.into_analog();
    // vol pot pb1
    let mut volpot = gpiob.pb1.into_analog();
    // btn pb14
    let button = gpiob.pb14.into_pull_up_input();
    let mut delay = delay::Delay::new(cp.SYST, ccdr.clocks);

    let mut adc = adc::Adc::adc1(
        dp.ADC1,
        4.MHz(),
        &mut delay,
        ccdr.peripheral.ADC12,
        &ccdr.clocks,
    )
    .enable();
    adc.set_resolution(adc::Resolution::TwelveBit);
    adc.set_sample_time(adc::AdcSampleTime::T_64);

    let mut led = gpioc.pc7.into_push_pull_output();

    let mut vcom_tim = dp.TIM2.timer(1.Hz(), ccdr.peripheral.TIM2, &ccdr.clocks);

    let mut col: bool = false;

    let seed1: u32 = adc.read(&mut pot1).unwrap();
    let seed2: u32 = adc.read(&mut pot2).unwrap();
    let seed1 = seed1 as u16;
    let seed2 = seed2 as u16;
    let mut prev_mapped: i32 = angle(seed1, seed2);
    let mut octave: i32 = 0;

    let phase_per_hz = (1u64 << 32) as f32 / SAMPLE_RATE.raw() as f32;
    let mut base_inc = [0u32; PENTA_LEN];
    for i in 0..PENTA_LEN {
        base_inc[i] = (ROOT_FREQ * PENTATONIC[i] * phase_per_hz) as u32;
    }

    let mut phase_inc: u32 = 0;
    let mut n: u32 = 0;

    let mut wave_idx: usize = 0;
    let mut button_pressed = button.is_low();
    let mut button_debounce: u32 = 0;

    let mut amplitude: f32;
    loop {
        if vcom_tim.wait().is_ok() {
            for i in 0..400 {
                for j in 0..240 {
                    let add = (j / 24) % 2;
                    let set = ((i / 16) + add) % 2 == 0;
                    sharp_driver.set_pixel(i, j, set == col);
                }
            }
            col = !col;
            sharp_driver.swap_vcom();
            while let Some(b) = sharp_driver.next_dirty_bytes() {
                sharp_spi.write(b).unwrap();
            }
        }
        let raw_pressed = button.is_low();
        if raw_pressed != button_pressed {
            button_debounce += 1;
            if button_debounce >= DEBOUNCE_SAMPLES {
                button_pressed = raw_pressed;
                button_debounce = 0;
                if button_pressed {
                    wave_idx = (wave_idx + 1) % WAVES.len();
                }
            }
        } else {
            button_debounce = 0;
        }

        n = n.wrapping_add(1);
        if n & 0xFF == 0 {
            let val1: u32 = adc.read(&mut pot1).unwrap();
            let val2: u32 = adc.read(&mut pot2).unwrap();
            let val3: u32 = adc.read(&mut volpot).unwrap();
            let val1 = val1 as u16;
            let val2 = val2 as u16;
            let val3 = val3 as u16;
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
            cortex_m::interrupt::free(|cs| {
                let mut osc = AUDIO.borrow(cs).borrow_mut();
                osc.wave_idx = wave_idx;
                osc.phase_inc = phase_inc;
                osc.amplitude = amplitude;
            });
        }
    }
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
        buf[i * 2 + 0] = s as u32;
        buf[i * 2 + 1] = s as u32;
    }
}

#[interrupt]
fn DMA1_STR0() {
    static mut TRANSFER: Option<I2sDma> = None;
    static mut LAST_FILLED: Option<dma::CurrentBuffer> = None;

    let mut osc = cortex_m::interrupt::free(|cs| *AUDIO.borrow(cs).borrow());
    let transfer = TRANSFER.get_or_insert_with(|| {
        cortex_m::interrupt::free(|cs| {
            AUDIO_TRANSFER
                .borrow(cs)
                .replace(None)
                .unwrap_or_else(|| panic!("DMA IRQ ran before its transfer was installed"))
        })
    });
    let filled = unsafe {
        transfer.next_transfer_with(|buf, current, remaining| {
            let _ = remaining;
            fill(buf, &mut osc);
            (buf, current)
        })
    };

    match filled {
        Ok(half) => {
            if *LAST_FILLED == Some(half) {
                panic!("DMA completed the same double buffer twice in a row");
            }
            *LAST_FILLED = Some(half);
        }
        Err(dma::DMAError::NotReady) => {
            panic!("DMA IRQ without a completed transfer");
        }
        Err(dma::DMAError::SmallBuffer) => {
            panic!("DMA replacement buffer length changed");
        }
        Err(dma::DMAError::Overflow) => {
            panic!("DMA overran the audio ISR");
        }
    }

    cortex_m::interrupt::free(|cs| AUDIO.borrow(cs).borrow_mut().phase = osc.phase);
}
