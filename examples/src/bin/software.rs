#![deny(warnings)]
#![no_main]
#![no_std]

use panic_semihosting as _;
use rtic;

#[rtic::app(device = stm32f4::stm32f401, dispatchers = [EXTI0, EXTI1])]
mod app {
    use cortex_m::peripheral::syst::SystClkSource;
    use cortex_m_rtic_trace::{
        self, trace, GlobalTimestampOptions, LocalTimestampOptions, TimestampClkSrc,
        TraceConfiguration, TraceProtocol,
    };

    #[shared]
    struct Shared {}

    #[local]
    struct Local {}

    #[init]
    fn init(mut ctx: init::Context) -> (Shared, Local, init::Monotonics) {
        ctx.core.SYST.set_clock_source(SystClkSource::Core);
        ctx.core.SYST.set_reload(16_000_000); // period = 1s

        // Allow debugger to attach while sleeping (WFI)
        ctx.device.DBGMCU.cr.modify(|_, w| {
            w.dbg_sleep().set_bit();
            w.dbg_standby().set_bit();
            w.dbg_stop().set_bit()
        });

        // flip device-specific master swtich for tracing
        #[rustfmt::skip]
        ctx.device.DBGMCU.cr.modify(
            |_, w| unsafe {
                w.trace_ioen().set_bit() // master enable for tracing
                 .trace_mode().bits(0b00) // TRACE pin assignment for async mode (SWO)
            },
        );

        // setup software tracing
        cortex_m_rtic_trace::configure(
            &mut ctx.core.DCB,
            &mut ctx.core.TPIU,
            &mut ctx.core.DWT,
            &mut ctx.core.ITM,
            1, // task enter DWT comparator ID
            2, // task exit DWT comparator ID
            &TraceConfiguration {
                delta_timestamps: LocalTimestampOptions::Enabled,
                absolute_timestamps: GlobalTimestampOptions::Disabled,
                timestamp_clk_src: TimestampClkSrc::AsyncTPIU,
                tpiu_freq: 16_000_000, // Hz
                tpiu_baud: 115_200,    // B/s
                protocol: TraceProtocol::AsyncSWONRZ,
            },
        )
        .unwrap();

        sw_task::spawn().unwrap();

        (Shared {}, Local {}, init::Monotonics())
    }

    #[task]
    #[trace]
    fn sw_task(_: sw_task::Context) {}
}
