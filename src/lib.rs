#![no_std]

pub use rtic_trace_macros::trace;

// TODO is there an even better way to store this?
static mut WATCH_VARIABLE: u32 = 0;

pub mod setup {
    use cortex_m::peripheral as Core;
    use cortex_m::peripheral::{
        dwt::{AccessType, ComparatorAddressSettings, ComparatorFunction, EmitOption},
        tpiu::TraceProtocol,
    };
    use stm32f4::stm32f401 as Device;

    /// Setup all core peripherals for task tracing.
    // TODO add option to enable/disable global/local timestamps
    pub fn core_peripherals(
        dcb: &mut Core::DCB,
        tpiu: &mut Core::TPIU,
        dwt: &mut Core::DWT,
        itm: &mut Core::ITM,
    ) {
        // TODO check feature availability

        // enable tracing
        dcb.enable_trace();

        tpiu.set_swo_baud_rate(16_000_000, 115_200);
        tpiu.set_trace_output_protocol(TraceProtocol::AsyncSWONRZ);
        tpiu.enable_continuous_formatting(false); // drops ETM packets

        dwt.enable_exception_tracing(true);
        dwt.enable_pc_samples(false);

        itm.unlock();
        // itm.configure(ITMSettings {
        //     enable: true, // ITMENA
        //     enable_local_timestamps: true, // TSENA XXX also take prescalar?
        //     forward_dwt: true, // TXENA
        //     global_timestamps: GlobalTimestamps::Every8192Cycles, // GTSFREQ
        //     bus_id: 1, // TraceBusID
        //     // XXX What about SYNCENA, SWOENA?
        // });

        // TODO PR new functions to cortex-m
        unsafe {
            itm.lar.write(0xc5acce55); // unlock ITM register
            itm.tcr.modify(|mut r| {
                r |= 1 << 0; // ITMENA: master enable
                r |= 1 << 1; // TSENA: enable local timestamps
                r |= 1 << 3; // TXENA: forward DWT event packets to ITM
                r |= 0b00 << 10; // GTSFREQ: disable global timestamps
                r |= 1 << 16; // TraceBusID=1
                r
            });
        }
    }

    pub fn device_peripherals(dbgmcu: &mut Device::DBGMCU) {
        #[rustfmt::skip]
        dbgmcu.cr.modify(
            |_, w| unsafe {
                w.trace_ioen().set_bit() // master enable for tracing
                 .trace_mode().bits(0b00) // TRACE pin assignment for async mode (SWO)
            },
        );
    }

    pub fn assign_dwt_unit(dwt: &Core::dwt::Comparator) {
        let watch_address: u32 = unsafe { &super::WATCH_VARIABLE as *const _ } as u32;
        // TODO do we need to clear the MATCHED, bit[24] after every match?
        dwt.configure(ComparatorFunction::Address(ComparatorAddressSettings {
            address: watch_address,
            mask: 0,
            emit: EmitOption::Data,
            access_type: AccessType::WriteOnly,
        }))
        .unwrap(); // NOTE safe: valid (emit, access_type) used
    }
}

#[inline]
pub fn set_current_task_id(id: u32) {
    unsafe {
        WATCH_VARIABLE = id;
    }
}
