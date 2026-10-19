// Copyright 2026 BoxLite Contributors
// SPDX-License-Identifier: Apache-2.0

//! The i8042 controller, reduced to the CPU reset that ends an x86_64 box.
//!
//! BoxLite's guest agent ends every box with `reboot()`, and an x86_64 guest
//! without ACPI resets by writing `0xFE` to port `0x64`. M1 configures no
//! guest keyboard, so `vm-superio`'s reset-only i8042 is the whole device:
//! every read is `0`, which also keeps the input-buffer-full bit clear for
//! the kernel's `kb_wait` before it writes the command.
//!
//! The reset sets a flag rather than stopping anything. The vCPU loop must
//! check it right after dispatching each port write, before re-entering the
//! guest: Linux keeps retrying and then falls back to other reset methods
//! (port `0xCF9`, a triple fault) if the vCPU runs on.

use std::sync::{Arc, atomic::AtomicBool};

use super::{ResetTrigger, registers};
use crate::bus::BusDevice;

/// The reset-only keyboard controller.
pub struct I8042 {
    inner: vm_superio::I8042Device<ResetTrigger>,
}

impl I8042 {
    /// The data port, the base of the device's window.
    pub const PORT_BASE: u16 = 0x60;
    /// One window spans the data port and the command port at offset 4, as
    /// Firecracker registers it; KVM's in-kernel PIT claims port `0x61`
    /// before it reaches the bus.
    pub const PORT_WINDOW: u16 = 5;

    /// Creates a controller that sets `reset_requested` on the reset command.
    pub fn new(reset_requested: Arc<AtomicBool>) -> Self {
        Self {
            inner: vm_superio::I8042Device::new(ResetTrigger::new(reset_requested)),
        }
    }
}

impl BusDevice for I8042 {
    fn read(&mut self, offset: u64, data: &mut [u8]) {
        for (byte, register) in data.iter_mut().zip(registers(offset)) {
            *byte = register.map_or(0, |register| self.inner.read(register));
        }
    }

    fn write(&mut self, offset: u64, data: &[u8]) {
        for (byte, register) in data.iter().zip(registers(offset)) {
            if let Some(register) = register {
                // `ResetTrigger` cannot fail.
                let _ = self.inner.write(register, *byte);
            }
        }
    }
}
