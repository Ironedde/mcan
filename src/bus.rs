//! Pad declarations for the CAN buses

use crate::filter::Filters;
use crate::messageram::SharedMemoryInner;
use crate::reg::{ecr::R as ECR, psr::R as PSR};
use crate::rx_dedicated_buffers::RxDedicatedBuffer;
use crate::rx_fifo::{Fifo0, Fifo1, RxFifo};
use crate::tx_buffers::Tx;
use crate::tx_event_fifo::TxEventFifo;
use core::convert::From;
use core::fmt::{self, Debug};

use super::{
    config::{
        bus::{CanConfig, CanFdMode, InterruptConfiguration, TestMode},
        RamConfig,
    },
    message::AnyMessage,
    messageram::{Capacities, SharedMemory},
};
use fugit::{HertzU32, RateExtU32};
use generic_array::typenum::Unsigned;

/// Printable PSR field
pub struct ProtocolStatus(pub PSR);

impl From<PSR> for ProtocolStatus {
    fn from(value: PSR) -> Self {
        Self(value)
    }
}

impl Debug for ProtocolStatus {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> fmt::Result {
        let psr = &self.0;

        f.debug_struct("ProtocolStatus")
            .field("tdcv", &psr.tdcv().bits())
            .field("pxe", &psr.pxe().bits())
            .field("rfdf", &psr.rfdf().bits())
            .field("rbrs", &psr.rbrs().bits())
            .field("resi", &psr.resi().bits())
            .field("dlec", &psr.dlec().bits())
            .field("bo", &psr.bo().bits())
            .field("ew", &psr.ew().bits())
            .field("ep", &psr.ep().bits())
            .field("act", &psr.act().bits())
            .field("lec", &psr.lec().bits())
            .finish()
    }
}

/// Printable ECR field
pub struct ErrorCounters(pub ECR);

impl From<ECR> for ErrorCounters {
    fn from(value: ECR) -> Self {
        Self(value)
    }
}

impl Debug for ErrorCounters {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> fmt::Result {
        let ecr = &self.0;

        f.debug_struct("ErrorCounters")
            .field("cel", &ecr.cel().bits())
            .field("rec", &ecr.rec().bits())
            .field("rp", &ecr.rp().bit())
            .field("tec", &ecr.tec().bits())
            .finish()
    }
}

/// Errors that may during configuration
#[derive(Debug)]
pub enum ConfigurationError {
    /// Divider is too large for the peripheral
    InvalidDivider(f32),
    /// Generating the bitrate would require a division by less than one
    ZeroDivider,
    /// Input clock / divider yields a poor integer division
    BitTimeRounding(HertzU32),
    /// Output clock is not supposed to be running
    StoppedOutputClock,
    /// The bit-time quanta is zero
    ZeroQuanta,
    /// Divider is f32 NaN
    DividerIsNaN,
    /// Divider is f32 Inf
    DividerIsInf,
    /// Time stamp prescaler value is not in the range [1, 16]
    InvalidTimeStampPrescaler,
    /// The provided memory is not addressable by the peripheral.
    MemoryNotAddressable,
}

/// Index is out of bounds
pub struct OutOfBounds;

/// Token for identifying bus during runtime
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BusSlot {
    /// Peripheral 0
    Can0,
    /// Peripheral 1
    Can1,
}

/// Common CANbus functionality
/// TODO: build interrupt struct around this
pub trait CanBus {
    /// Read error counters
    fn error_counters(&self) -> ErrorCounters;
    /// Read additional status information
    fn protocol_status(&self) -> ProtocolStatus;
    /// Get current time
    fn ts_count(&self) -> u16;
}

/// A CAN bus that is not in configuration mode (CCE=0). Some errors (including
/// Bus_Off) can asynchronously stop bus operation (INIT=1), which will require
/// user intervention to reactivate the bus to resume sending and receiving
/// messages.
pub struct Can<'a, Id, D, C: Capacities> {
    /// Controls enabling and line selection of interrupts.
    pub interrupts: InterruptConfiguration<Id>,
    pub rx_fifo_0: RxFifo<'a, Fifo0, Id, C::RxFifo0Message>,
    pub rx_fifo_1: RxFifo<'a, Fifo1, Id, C::RxFifo1Message>,
    pub rx_dedicated_buffers: RxDedicatedBuffer<'a, Id, C::RxBufferMessage>,
    pub tx: Tx<'a, Id, C>,
    pub tx_event_fifo: TxEventFifo<'a, Id>,

    /// Implementation details. The field is public to allow destructuring.
    pub internals: Internals<'a, Id, D>,
}

/// Implementation details.
pub struct Internals<'a, Id, D> {
    /// CAN bus peripheral
    can: crate::reg::Can<Id>,
    dependencies: D,
    filters: Filters<'a, Id>,
}

/// A CAN bus in configuration mode. Before messages can be sent and received,
/// it needs to be [`Self::finalize`]d.
pub struct CanConfigurable<'a, Id, D, C: Capacities>(
    /// The type invariant of CCE=0 is broken while this is wrapped.
    Can<'a, Id, D, C>,
);

impl<'a, Id: crate::CanId, D: crate::Dependencies<Id>, C: Capacities>
    CanConfigurable<'a, Id, D, C>
{
    /// Raw access to the registers.
    ///
    /// # Safety
    /// The abstraction assumes that it has exclusive ownership of the
    /// registers. Direct access can break such assumptions. Direct access
    /// can break assumptions
    pub unsafe fn registers(&self) -> &crate::reg::Can<Id> {
        self.0.registers()
    }

    /// Allows reconfiguring the acceptance filters.
    pub fn filters(&mut self) -> &mut Filters<'a, Id> {
        &mut self.0.internals.filters
    }

    /// Apply parameters from a bus config struct
    fn apply_bus_config(
        &mut self,
        config: &CanConfig,
        freq: HertzU32,
    ) -> Result<(), ConfigurationError> {
        if !(1..=16).contains(&config.timing.ts_prescale) {
            return Err(ConfigurationError::InvalidTimeStampPrescaler);
        }

        // Baud rate
        // TODO: rewrite this somewhat when we're required to implement variable data
        // rate!
        let c = self.0.internals.dependencies.can_clock().to_Hz();
        let f = freq.to_Hz();
        let q = config.timing.quanta();

        // Sanity check input parameters
        if f == 0 {
            return Err(ConfigurationError::StoppedOutputClock);
        } else if q == 0 {
            return Err(ConfigurationError::ZeroQuanta);
        }

        // Calculate divider
        let cf: f32 = c as f32;
        let ff: f32 = f as f32;
        let qf: f32 = q as f32;
        let divider = cf / (ff * qf);

        // Convert divider to u32
        // Safety: criterion and tested above, the divider is:
        //   * Not `NaN`
        //   * Not `Inf`
        //   * Divider smaller than 512 implies that it is smaller than max u32 and
        //     since it is generated from non-negative inputs it should be in range for
        //     u32
        let divider: u32 = if divider.is_nan() {
            return Err(ConfigurationError::DividerIsNaN);
        } else if divider.is_infinite() {
            return Err(ConfigurationError::DividerIsInf);
        } else if divider < 1.0f32 {
            // Dividers of < 1 round down to 0
            return Err(ConfigurationError::ZeroDivider);
        } else if divider >= 512.0f32 {
            return Err(ConfigurationError::InvalidDivider(divider));
        } else {
            unsafe { f32::to_int_unchecked(divider) }
        };

        // Compare the real output to the expected output
        let real_output = c / (divider * q as u32);

        if real_output != f {
            return Err(ConfigurationError::BitTimeRounding(real_output.Hz()));
        } else {
            unsafe {
                self.0.internals.can.nbtp.write(|w| {
                    w.nsjw()
                        .bits(config.timing.sjw)
                        .ntseg1()
                        .bits(config.timing.phase_seg_1)
                        .ntseg2()
                        .bits(config.timing.phase_seg_2)
                        .nbrp()
                        .bits((divider - 1) as u16)
                });

                self.0.internals.can.tscc.write(|w| {
                    w.tss()
                        .bits(config.timing.ts_select.into())
                        // Prescaler is 1 + tcp value.
                        .tcp()
                        .bits(config.timing.ts_prescale - 1)
                });

                // CAN-FD operation
                self.0
                    .internals
                    .can
                    .cccr
                    .modify(|_, w| w.fdoe().bit(config.fd_mode.clone().into()));
                // HACK: Data bitrate is 1Mb/s
                self.0.internals.can.dbtp.modify(|_, w| w.dbrp().bits(2));
                self.0
                    .internals
                    .can
                    .cccr
                    .modify(|_, w| w.brse().bit(config.bit_rate_switching));
                // Global filter options
                self.0.internals.can.gfc.write(|w| {
                    w.anfs()
                        .bits(config.nm_std.clone().into())
                        .anfe()
                        .bits(config.nm_ext.clone().into())
                });

                // Configure test/loopback mode
                self.set_test(config.test.clone());
            }

            Ok(())
        }
    }

    /// Apply parameters from a ram config struct
    ///
    /// Ensuring that the RAM config struct is properly defined is basically our
    /// only safeguard keeping the bus operational. Apart from that, the
    /// memory RAM is largely unchecked and an improperly configured linker
    /// script could interfere with bus operations.
    fn apply_ram_config(
        can: &crate::reg::Can<Id>,
        mem: &SharedMemoryInner<C>,
        config: &RamConfig,
    ) -> Result<(), ConfigurationError> {
        if !mem.is_addressable() {
            return Err(ConfigurationError::MemoryNotAddressable);
        }

        unsafe {
            // Standard id
            can.sidfc.write(|w| {
                w.flssa()
                    .bits(&mem.filters_standard as *const _ as u16)
                    .lss()
                    .bits(mem.filters_standard.len() as u8)
            });

            // Extended id
            can.xidfc.write(|w| {
                w.flesa()
                    .bits(&mem.filters_extended as *const _ as u16)
                    .lse()
                    .bits(mem.filters_extended.len() as u8)
            });

            // RX buffers
            can.rxbc
                .write(|w| w.rbsa().bits(&mem.rx_dedicated_buffers as *const _ as u16));

            // Data field size for buffers and FIFOs
            can.rxesc.write(|w| {
                w.rbds()
                    .bits(C::RxBufferMessage::REG)
                    .f0ds()
                    .bits(C::RxFifo0Message::REG)
                    .f1ds()
                    .bits(C::RxFifo1Message::REG)
            });

            //// RX FIFO 0
            can.rxf0.c.write(|w| {
                w.fom()
                    .bit(config.rxf0.mode.clone().into())
                    .fwm()
                    .bits(config.rxf0.watermark)
                    .fs()
                    .bits(mem.rx_fifo_0.len() as u8)
                    .fsa()
                    .bits(&mem.rx_fifo_0 as *const _ as u16)
            });

            //// RX FIFO 1
            can.rxf1.c.write(|w| {
                w.fom()
                    .bit(config.rxf1.mode.clone().into())
                    .fwm()
                    .bits(config.rxf1.watermark)
                    .fs()
                    .bits(mem.rx_fifo_1.len() as u8)
                    .fsa()
                    .bits(&mem.rx_fifo_1 as *const _ as u16)
            });

            // TX buffers
            can.txbc.write(|w| {
                w.tfqm()
                    .bit(config.txbc.mode.clone().into())
                    .tfqs()
                    .bits(<C::TxBuffers as Unsigned>::U8 - <C::DedicatedTxBuffers as Unsigned>::U8)
                    .ndtb()
                    .bits(<C::DedicatedTxBuffers as Unsigned>::U8)
                    .tbsa()
                    .bits(&mem.tx_buffers as *const _ as u16)
            });

            // TX element size config
            can.txesc.write(|w| w.tbds().bits(C::TxMessage::REG));

            // TX events
            can.txefc.write(|w| {
                w.efwm()
                    .bits(config.txefc.watermark)
                    .efs()
                    .bits(mem.tx_event_fifo.len() as u8)
                    .efsa()
                    .bits(&mem.tx_event_fifo as *const _ as u16)
            });
        }
        Ok(())
    }

    /// Configure test mode
    pub fn set_fd(&mut self, fd: CanFdMode) {
        self.0
            .internals
            .can
            .cccr
            .modify(|_, w| w.fdoe().bit(fd.clone().into()));
    }

    /// Configure test mode
    fn set_test(&mut self, test: TestMode) {
        match test {
            TestMode::Disabled => {
                self.0.internals.can.cccr.modify(|_, w| w.test().bit(false));
                self.0.internals.can.test.modify(|_, w| w.lbck().bit(false));
            }
            TestMode::Loopback => {
                self.0.internals.can.cccr.modify(|_, w| w.test().bit(true));
                self.0.internals.can.test.modify(|_, w| w.lbck().bit(true));
            }
        }
    }
}

impl<Id: crate::CanId, D: crate::Dependencies<Id>, C: Capacities> Can<'_, Id, D, C> {
    /// Raw access to the registers.
    ///
    /// # Safety
    /// The abstraction assumes that it has exclusive ownership of the
    /// registers. Direct access can break such assumptions. Direct access
    /// can break assumptions
    pub unsafe fn registers(&self) -> &crate::reg::Can<Id> {
        &self.internals.can
    }

    /// Switches between "Software Initialization" mode and "Normal Operation".
    /// In Software Initialization, messages are not received or transmitted.
    /// Configuration cannot be changed. In Normal Operation, messages can
    /// be transmitted and received.
    pub fn set_init(&mut self, init: bool) {
        self.internals.can.cccr.modify(|_, w| w.init().bit(init));
        while self.internals.can.cccr.read().init().bit() != init {}
    }

    fn enable_configuration_change(&mut self) {
        self.set_init(true);
        self.internals.can.cccr.modify(|_, w| w.cce().set_bit());
        while !self.internals.can.cccr.read().cce().bit() {}
    }
}

impl<Id: crate::CanId, D: crate::Dependencies<Id>, C: Capacities> CanBus for Can<'_, Id, D, C> {
    fn error_counters(&self) -> ErrorCounters {
        self.internals.can.ecr.read().into()
    }

    fn protocol_status(&self) -> ProtocolStatus {
        self.internals.can.psr.read().into()
    }

    fn ts_count(&self) -> u16 {
        self.internals.can.tscv.read().tsc().bits()
    }
}

impl<'a, Id: crate::CanId, D: crate::Dependencies<Id>, C: Capacities> Can<'a, Id, D, C> {
    pub fn configure(mut self) -> CanConfigurable<'a, Id, D, C> {
        self.enable_configuration_change();
        CanConfigurable(self)
    }
}

impl<'a, Id: crate::CanId, D: crate::Dependencies<Id>, C: Capacities>
    CanConfigurable<'a, Id, D, C>
{
    /// Create new can peripheral.
    ///
    /// The hardware requires that SharedMemory is contained within the first
    /// 64K of system RAM. If this condition is not fulfilled, an error is
    /// returned.
    ///
    /// The returned peripheral is not operational; use [`Self::finalize`] to
    /// finish configuration and start transmitting and receiving.
    pub fn new(
        dependencies: D,
        freq: HertzU32,
        can_cfg: CanConfig,
        ram_cfg: RamConfig,
        memory: &'a mut SharedMemory<C>,
    ) -> Result<Self, ConfigurationError> {
        // Safety:
        // Since `dependencies` field implies ownership of the HW register pointed to by
        // `Id: CanId`, `can` has a unique access to it
        let can = unsafe { crate::reg::Can::<Id>::new() };

        let memory = memory.init();
        Self::apply_ram_config(&can, memory, &ram_cfg)?;

        let mut bus = Can {
            // Safety: Since `Can::new` takes a PAC singleton, it can only be called once. Then no
            // duplicates will be constructed. The registers that are delegated to these components
            // should not be touched by any other code. This has to be upheld by all code that has
            // access to the register block.
            interrupts: unsafe { InterruptConfiguration::new() },
            rx_fifo_0: unsafe { RxFifo::new(&mut memory.rx_fifo_0) },
            rx_fifo_1: unsafe { RxFifo::new(&mut memory.rx_fifo_1) },
            rx_dedicated_buffers: unsafe {
                RxDedicatedBuffer::new(&mut memory.rx_dedicated_buffers)
            },
            tx: unsafe { Tx::new(&mut memory.tx_buffers) },
            tx_event_fifo: unsafe { TxEventFifo::new(&mut memory.tx_event_fifo) },
            internals: Internals {
                can,
                dependencies,
                // Safety: The memory is zeroed by `memory.init`, so all filters are initially
                // disabled.
                filters: unsafe {
                    Filters::new(&mut memory.filters_standard, &mut memory.filters_extended)
                },
            },
        }
        .configure();

        bus.apply_bus_config(&can_cfg, freq)?;
        Ok(bus)
    }

    /// Locks the configuration and enters normal operation.
    pub fn finalize(mut self) -> Can<'a, Id, D, C> {
        self.0.set_init(false);
        self.0
    }
}
