// Copyright 2026 BoxLite Contributors
// SPDX-License-Identifier: Apache-2.0

//! x86_64 legacy devices for M1 first boot (design, "Devices by milestone").
//!
//! The 8250 serial and the reset-only i8042 are `vm-superio`'s, behind
//! BoxLite's port bus: the adapters here translate bus offsets, route the
//! serial's interrupt through [`IrqSender`], and turn the i8042's CPU-reset
//! command into a flag for the vCPU loop. The module tree is
//! architecture-gated where `lib.rs` declares it.

pub mod i8042;
pub mod rtc;
pub mod serial;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use vm_superio::Trigger;

use crate::irq::IrqSender;

/// Pulses one interrupt line each time a `vm-superio` device signals.
///
/// `vm-superio` calls `trigger` once per new interrupt condition, as an
/// eventfd-backed irqfd would see it, so each call is an edge.
pub struct IrqTrigger {
    irq: IrqSender,
    line: u32,
}

impl IrqTrigger {
    /// Signals `line` through `irq`.
    pub fn new(irq: IrqSender, line: u32) -> Self {
        Self { irq, line }
    }
}

impl Trigger for IrqTrigger {
    type E = boxlite_hypervisor::Error;

    fn trigger(&self) -> Result<(), Self::E> {
        self.irq.trigger_edge(self.line)
    }
}

/// Records a guest reset request for the vCPU loop.
pub struct ResetTrigger(Arc<AtomicBool>);

impl ResetTrigger {
    /// Sets `requested` when the guest asks for a reset.
    pub fn new(requested: Arc<AtomicBool>) -> Self {
        Self(requested)
    }
}

impl Trigger for ResetTrigger {
    type E = std::convert::Infallible;

    fn trigger(&self) -> Result<(), Self::E> {
        self.0.store(true, Ordering::SeqCst);
        Ok(())
    }
}

/// Splits a bus access into the single-register accesses `vm-superio`
/// takes: byte `i` of the access goes to register `offset + i`. An offset
/// the device cannot name reads `0` and drops writes, so no access panics.
fn registers(offset: u64) -> impl Iterator<Item = Option<u8>> {
    (0..).map(move |i: u64| {
        offset
            .checked_add(i)
            .and_then(|register| u8::try_from(register).ok())
    })
}

#[cfg(test)]
#[path = "../../tests/devices/mod.rs"]
mod tests;
