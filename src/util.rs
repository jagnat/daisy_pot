use embassy_stm32::{pac, rcc, Peripherals, Config, time::{Hertz}};

pub fn config_plls(cfg: &mut Config) {
    cfg.rcc.hse = Some(rcc::Hse { freq: Hertz::mhz(16), mode:rcc::HseMode::Oscillator} );
    cfg.rcc.voltage_scale = rcc::VoltageScale::Scale0;

    cfg.rcc.pll1 = Some(rcc::Pll {
        source: rcc::PllSource::HSE,
        prediv: rcc::PllPreDiv::DIV4, // 16 mhz / 4 = 4 mhz
        mul: rcc::PllMul::MUL240, // 4 * 240 = 960 mhz
        divp: Some(rcc::PllDiv::DIV2), // 960 / 2 = 480 mhz (system clock)
        divq: Some(rcc::PllDiv::DIV40),  // 960 / 40 = 24 mhz for spi
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

    // use external crystal for adc
    cfg.rcc.mux.persel = pac::rcc::vals::Persel::HSE;
    cfg.rcc.mux.adcsel = pac::rcc::vals::Adcsel::PER;

    // use pll1 Q for spi
    cfg.rcc.mux.spi123sel = pac::rcc::vals::Saisel::PLL1_Q;

    // seed3's TAC5242 codec is clocked by SAI1, give it 49.152 MHz so
    // DIV4 gives us 12.288 MHz clock (256 * 48 kHz)
    cfg.rcc.mux.sai1sel = pac::rcc::vals::Saisel::PLL3_P;
    cfg.rcc.pll3 = Some(rcc::Pll {
        source: rcc::PllSource::HSE,
        prediv: rcc::PllPreDiv::DIV25, // 16 MHz / 25 = 640 kHz
        mul: rcc::PllMul::MUL384,      // 640 kHz * 384 = 245.76 MHz
        divp: Some(rcc::PllDiv::DIV5), // 245.76 MHz / 5 = 49.152 MHz
        divq: None,
        divr: None,
    });
}

pub fn assert_pll(p: &Peripherals) {
    let clocks = rcc::clocks(&p.RCC);
    assert_eq!(clocks.sys.to_hertz(), Some(Hertz::mhz(480)));
    assert_eq!(clocks.hclk1.to_hertz(), Some(Hertz::mhz(240)));
    assert_eq!(clocks.pclk1.to_hertz(), Some(Hertz::mhz(120)));
}

