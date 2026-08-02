#![no_main]
#![no_std]

use cortex_m_rt::entry;
use stm32h7xx_hal::gpio::Speed;
use stm32h7xx_hal::spi;
use stm32h7xx_hal::time;
use stm32h7xx_hal::{pac, prelude::*, rcc::PllConfigStrategy};
use crate::ls027b4dh01::SharpDisplayDriver;
use crate::luts::*;

mod ls027b4dh01;
mod luts;
mod font;
mod panic;

#[entry]
fn main() -> ! {
    let cp = cortex_m::Peripherals::take().unwrap();
    let dp = pac::Peripherals::take().unwrap();

    let pwr = dp.PWR.constrain().vos0(&dp.SYSCFG);
    let pwrcfg = pwr.freeze();

    let mut rcc = dp.RCC.constrain();
    let ccdr = rcc.use_hse(16.MHz()).pll1_strategy(PllConfigStrategy::Iterative).pll1_q_ck(96.MHz()).sys_ck(480.MHz()).freeze(pwrcfg, &dp.SYSCFG);

    panic::publish_sys_hz(ccdr.clocks.sys_ck().raw());
    assert!(ccdr.clocks.sys_ck().raw() == 480_000_000);

    let gpioa = dp.GPIOA.split(ccdr.peripheral.GPIOA);
    let gpiob = dp.GPIOB.split(ccdr.peripheral.GPIOB);
    let gpioc = dp.GPIOC.split(ccdr.peripheral.GPIOC);
    let gpiog = dp.GPIOG.split(ccdr.peripheral.GPIOG);

    let sharp_freq: time::Hertz = 12.MHz();
    let sharp_driver = cortex_m::singleton!(: SharpDisplayDriver = SharpDisplayDriver::new()).unwrap();
    let sharp_sck = gpiog.pg11.into_alternate().speed(Speed::VeryHigh);
    let sharp_mosi = gpiob.pb5.into_alternate().speed(Speed::VeryHigh);
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

    let mut led = gpioc.pc7.into_push_pull_output();

    let mut delay = cp.SYST.delay(ccdr.clocks);
    let mut vcom_tim = dp.TIM2.timer(1.Hz(), ccdr.peripheral.TIM2, &ccdr.clocks);
    let mut col: bool = false;
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
    }
}

