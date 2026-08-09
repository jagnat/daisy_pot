//! Panic indicator on the user LED.
//!
//! With no debug probe attached and no serial console, PC7 is the only output channel a
//! panic can reach. Pattern is three short flashes then a one second pause, which no
//! running state produces.

use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU32, Ordering};

use cortex_m::asm;
use embassy_stm32::pac;

/// Defaults to the HSI rate the core boots at, so a panic before clock init still blinks
/// at roughly the right speed.
static SYS_HZ: AtomicU32 = AtomicU32::new(64_000_000);

pub fn publish_sys_hz(hz: u32) {
    SYS_HZ.store(hz, Ordering::Relaxed);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    cortex_m::interrupt::disable();

    // Direct PAC access is appropriate here: once panicking, normal peripheral ownership
    // no longer matters. This is idempotent and also works before Embassy initialization.
    pac::RCC.ahb4enr().modify(|w| w.set_gpiocen(true));
    pac::GPIOC
        .otyper()
        .modify(|w| w.set_ot(7, pac::gpio::vals::Ot::PUSH_PULL));
    pac::GPIOC
        .moder()
        .modify(|w| w.set_moder(7, pac::gpio::vals::Moder::OUTPUT));

    let hz = SYS_HZ.load(Ordering::Relaxed);
    let flash = hz / 10;

    loop {
        for _ in 0..3 {
            pac::GPIOC.bsrr().write(|w| w.set_bs(7, true));
            asm::delay(flash);
            pac::GPIOC.bsrr().write(|w| w.set_br(7, true));
            asm::delay(flash);
        }
        asm::delay(hz);
    }
}
