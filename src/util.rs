use embassy_stm32::{rcc, Peripherals, Config, time::{Hertz}};

pub fn config_pll(cfg: &mut Config) {
    cfg.rcc.hse = Some(rcc::Hse { freq: Hertz::mhz(16), mode:rcc::HseMode::Oscillator} );
    cfg.rcc.voltage_scale = rcc::VoltageScale::Scale0;

    cfg.rcc.pll1 = Some(rcc::Pll {
        source: rcc::PllSource::HSE,
        prediv: rcc::PllPreDiv::DIV4, // 16 mhz / 4 = 4 mhz
        mul: rcc::PllMul::MUL240, // 4 * 240 = 960 mhz
        divp: Some(rcc::PllDiv::DIV2), // 960 / 2 = 480 mhz (system clock)
        divq: None, 
        divr: None,
    });
    cfg.rcc.sys = rcc::Sysclk::PLL1_P;

    // domain 1 core pre = 480 / 1
    cfg.rcc.d1c_pre = rcc::AHBPrescaler::DIV1;
    // high performance bus predivide = 480 / 2 = 240 mhz
    cfg.rcc.ahb_pre = rcc::AHBPrescaler::DIV2;

    // peripheral busses =  hpre (240) / 2 = 120 mhz
    cfg.rcc.apb1_pre = rcc::APBPrescaler::DIV2;
    cfg.rcc.apb2_pre = rcc::APBPrescaler::DIV2;
    cfg.rcc.apb3_pre = rcc::APBPrescaler::DIV2;
    cfg.rcc.apb4_pre = rcc::APBPrescaler::DIV2;
}

pub fn assert_pll(p: &Peripherals) {
    let clocks = rcc::clocks(&p.RCC);
    assert_eq!(clocks.sys.to_hertz(), Some(Hertz::mhz(480)));
    assert_eq!(clocks.hclk1.to_hertz(), Some(Hertz::mhz(240)));
    assert_eq!(clocks.pclk1.to_hertz(), Some(Hertz::mhz(120)));
}
