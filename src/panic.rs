//! Panic indicator on the user LED.
//!
//! With no debug probe attached and no serial console, PC7 is the only output channel a
//! panic can reach. Pattern is three short flashes then a one second pause, which no
//! running state produces.

use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU32, Ordering};

use cortex_m::asm;
use stm32h7xx_hal::pac;

/// Defaults to the HSI rate the core boots at, so a panic before clock init still blinks
/// at roughly the right speed.
static SYS_HZ: AtomicU32 = AtomicU32::new(64_000_000);

pub fn publish_sys_hz(hz: u32) {
    SYS_HZ.store(hz, Ordering::Relaxed);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    // Stealing is sound here in the only sense that matters: nothing else will run again.
    // The HAL is unusable from a panic handler regardless, since splitting a GPIO port
    // needs an RCC ownership token this code has no way to obtain.
    let dp = unsafe { pac::Peripherals::steal() };

    // Idempotent, and covers a panic that happened before main configured the pin.
    dp.RCC.ahb4enr.modify(|_, w| w.gpiocen().set_bit());
    dp.GPIOC.moder.modify(|_, w| w.moder7().output());

    let hz = SYS_HZ.load(Ordering::Relaxed);
    let flash = hz / 10;

    loop {
        for _ in 0..3 {
            dp.GPIOC.bsrr.write(|w| w.bs7().set_bit());
            asm::delay(flash);
            dp.GPIOC.bsrr.write(|w| w.br7().set_bit());
            asm::delay(flash);
        }
        asm::delay(hz);
    }
}
