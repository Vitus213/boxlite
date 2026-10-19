// Copyright 2026 BoxLite Contributors
// SPDX-License-Identifier: Apache-2.0

//! Device interrupt assignment and routing.
//!
//! HVF and KVM provide the interrupt controller in the kernel (PIC/APIC on
//! x86_64, GICv3 on arm64; WHP (M10) will provide only local APICs), so the
//! VMM emulates none of it. A device raises or lowers its line through
//! [`IrqSender`], which is the named interrupt path from the device model
//! design (`docs/contributing/architecture/vmm/README.md`, "Interrupts").
//! Unparking halted vCPUs after an injection, which the design pairs with
//! this path, arrives with the vCPU workers.

use std::sync::Arc;

use boxlite_hypervisor::Result;

/// The injection path an [`IrqSender`] drives.
///
/// This is the narrow slice of [`boxlite_hypervisor::Vm`] that devices need.
/// `Vm` has an associated vCPU type, so devices cannot hold it as a trait
/// object; this trait can be. Every backend VM implements it through the
/// blanket impl below, and tests implement it directly with a recorder.
pub trait InterruptTarget: Send + Sync {
    /// Sets interrupt `line` to `level`.
    ///
    /// `line` is a GSI on x86_64 and a GIC SPI INTID (32 and up) on arm64.
    fn set_irq_line(&self, line: u32, level: bool) -> Result<()>;
}

impl<V: boxlite_hypervisor::Vm> InterruptTarget for V {
    fn set_irq_line(&self, line: u32, level: bool) -> Result<()> {
        boxlite_hypervisor::Vm::set_irq_line(self, line, level)
    }
}

/// A device's handle for raising its interrupt line, safe to clone and hold
/// across worker threads.
#[derive(Clone)]
pub struct IrqSender {
    target: Arc<dyn InterruptTarget>,
}

impl IrqSender {
    /// Wraps a backend's interrupt-injection path.
    pub fn new(target: Arc<dyn InterruptTarget>) -> Self {
        Self { target }
    }

    /// Holds level-triggered line `line` at `level`.
    ///
    /// The 8250 serial is level-triggered: raise on a pending condition and
    /// keep the line high until the guest service clears it.
    pub fn set_level(&self, line: u32, level: bool) -> Result<()> {
        self.target.set_irq_line(line, level)
    }

    /// Pulses edge-triggered line `line`: a set immediately followed by a
    /// clear, which is how the in-kernel controllers spell an edge (design,
    /// "Interrupts").
    ///
    /// If the clear fails after the set succeeded, the error still
    /// propagates: leaving the line high is louder than swallowing it, and
    /// the caller decides what a stuck line costs.
    pub fn trigger_edge(&self, line: u32) -> Result<()> {
        self.target.set_irq_line(line, true)?;
        self.target.set_irq_line(line, false)
    }
}

// The suite lives under `tests/` so it stays out of the counted source.
#[cfg(test)]
#[path = "../tests/irq/mod.rs"]
mod tests;
