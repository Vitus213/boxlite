// Copyright 2026 BoxLite Contributors
// SPDX-License-Identifier: Apache-2.0

//! The 16550A UART at PC ports `0x3F8`–`0x3FF` on GSI 4.
//!
//! The guest kernel's early console writes here, so the transmit path is a
//! byte sink (BoxLite's `console.log` later, a buffer in tests). The register
//! model is `vm-superio`'s 8250, the one Firecracker ships.

use std::io::Write;

use vm_superio::serial::NoEvents;

use super::{IrqTrigger, registers};
use crate::{bus::BusDevice, irq::IrqSender};

/// The serial console device.
pub struct Serial {
    inner: vm_superio::Serial<IrqTrigger, NoEvents, Box<dyn Write + Send>>,
}

impl Serial {
    /// The first of the eight consecutive ports the device occupies.
    pub const PORT_BASE: u16 = 0x3F8;
    /// The width of that port window.
    pub const PORT_WINDOW: u16 = 8;
    /// The interrupt line of the first PC serial port.
    pub const GSI: u32 = 4;

    /// Creates a port writing console output to `sink` and signalling on
    /// [`Self::GSI`] through `irq`.
    pub fn new(sink: Box<dyn Write + Send>, irq: IrqSender) -> Self {
        Self {
            inner: vm_superio::Serial::new(IrqTrigger::new(irq, Self::GSI), sink),
        }
    }

    /// Queues host input for the guest, returning how many bytes fit in the
    /// receive FIFO; the console bridge retries the rest once the guest
    /// drains it.
    pub fn enqueue_input(&mut self, bytes: &[u8]) -> usize {
        // A full FIFO or a failed interrupt leaves the bytes for the next
        // call: the guest polls LSR, and the bridge retries.
        self.inner.enqueue_raw_bytes(bytes).unwrap_or(0)
    }
}

impl BusDevice for Serial {
    fn read(&mut self, offset: u64, data: &mut [u8]) {
        for (byte, register) in data.iter_mut().zip(registers(offset)) {
            *byte = register.map_or(0, |register| self.inner.read(register));
        }
    }

    fn write(&mut self, offset: u64, data: &[u8]) {
        for (byte, register) in data.iter().zip(registers(offset)) {
            if let Some(register) = register {
                // A failing sink or interrupt injection drops the byte or the
                // edge: a port write has no error path back to the guest, and
                // the guest must not stall on host I/O.
                let _ = self.inner.write(register, *byte);
            }
        }
    }
}
