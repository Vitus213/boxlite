// Copyright 2026 BoxLite Contributors
// SPDX-License-Identifier: Apache-2.0

//! The machine's devices and the resources they hold.
//!
//! [`ChipsetBuilder`] is the one registry of port ranges, MMIO ranges and
//! interrupt lines, as OpenVMM's chipset builder is. Platform code adds each
//! device by name with a closure that claims the device's resources from a
//! [`Resources`] handle; `build` returns an immutable [`Chipset`], or every
//! conflict at once with both sides named. The chipset owns the devices and
//! is shared by the vCPU threads, which dispatch exits to it. It never
//! changes after the build: no milestone adds a bus device at run time.

use std::{
    collections::{BTreeMap, btree_map::Entry},
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use boxlite_hypervisor::VcpuExit;

use crate::{
    bus::{BusDevice, BusRange, Buses, Target},
    error::{Error, Result},
    events::EventSender,
    irq::{IrqChip, IrqLine},
};

/// Which address space a range lives in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BusKind {
    Mmio,
    #[cfg(target_arch = "x86_64")]
    Pio,
}

impl fmt::Display for BusKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Mmio => "MMIO",
            #[cfg(target_arch = "x86_64")]
            Self::Pio => "port",
        })
    }
}

/// A resource two parties claimed, or a claim the bus cannot hold.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Conflict {
    /// `device` claimed a range that `owner`, a device or a reserved window,
    /// already holds.
    Range {
        bus: BusKind,
        device: &'static str,
        base: u64,
        len: u64,
        owner: &'static str,
    },
    /// `device` claimed an empty or wrapping range.
    InvalidRange {
        bus: BusKind,
        device: &'static str,
        base: u64,
        len: u64,
    },
    /// `device` claimed an interrupt line that `owner` already holds.
    Line {
        line: u32,
        device: &'static str,
        owner: &'static str,
    },
}

impl fmt::Display for Conflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Range {
                bus,
                device,
                base,
                len,
                owner,
            } => write!(
                f,
                "{device} claims {bus} range {base:#x}+{len:#x}, held by {owner}"
            ),
            Self::InvalidRange {
                bus,
                device,
                base,
                len,
            } => write!(
                f,
                "{device} claims {bus} range {base:#x}+{len:#x}, which is empty or wraps"
            ),
            Self::Line {
                line,
                device,
                owner,
            } => write!(f, "{device} claims interrupt line {line}, held by {owner}"),
        }
    }
}

/// The host interrupt controller as the builder's lines see it: inert until
/// `build` succeeds, so a device constructor cannot raise a line for a
/// device that a later conflict discards.
struct GatedIrqChip {
    inner: Arc<dyn IrqChip>,
    armed: AtomicBool,
}

impl IrqChip for GatedIrqChip {
    fn set_line(&self, line: u32, level: bool) -> Result<()> {
        if !self.armed.load(Ordering::Acquire) {
            return Err(Error::LineNotArmed(line));
        }
        self.inner.set_line(line, level)
    }
}

/// One range a device claimed: where it sits, and the register offset the
/// device sees at its base.
struct Claim {
    bus: BusKind,
    base: u64,
    len: u64,
    offset: u64,
}

/// The resources a device constructor claims. Claims are recorded, not
/// checked, so the builder can report every conflict at once.
pub(crate) struct Resources {
    chip: Arc<GatedIrqChip>,
    events: EventSender,
    claims: Vec<Claim>,
    lines: Vec<u32>,
}

impl Resources {
    /// Claims `len` ports from `base`; the device sees offset 0 at `base`.
    #[cfg(target_arch = "x86_64")]
    pub(crate) fn pio(&mut self, base: u64, len: u64) {
        self.pio_at(base, len, 0);
    }

    /// Claims `len` ports from `base` as the device's registers from
    /// `offset`, for a controller whose ports are not contiguous.
    #[cfg(target_arch = "x86_64")]
    pub(crate) fn pio_at(&mut self, base: u64, len: u64, offset: u64) {
        self.claims.push(Claim {
            bus: BusKind::Pio,
            base,
            len,
            offset,
        });
    }

    /// Claims `len` bytes of MMIO from `base`; the device sees offset 0 at
    /// `base`.
    pub(crate) fn mmio(&mut self, base: u64, len: u64) {
        self.claims.push(Claim {
            bus: BusKind::Mmio,
            base,
            len,
            offset: 0,
        });
    }

    /// Claims interrupt `line`: a GSI on x86_64, an SPI on arm64. The device
    /// owns the returned line, which reaches the host only once `build`
    /// succeeds; a second claim of the same line fails the build.
    pub(crate) fn line(&mut self, line: u32) -> IrqLine {
        self.lines.push(line);
        IrqLine::new(self.chip.clone(), line)
    }

    /// The sender for the events that end the VM. A constructor keeps it for
    /// later and sends nothing.
    pub(crate) fn events(&self) -> EventSender {
        self.events.clone()
    }
}

/// Collects devices and their resources, then builds the [`Chipset`].
pub(crate) struct ChipsetBuilder {
    chip: Arc<GatedIrqChip>,
    events: EventSender,
    buses: Buses,
    lines: BTreeMap<u32, &'static str>,
    devices: Vec<Arc<Mutex<dyn BusDevice>>>,
    conflicts: Vec<Conflict>,
}

impl ChipsetBuilder {
    /// A builder whose devices raise interrupts on `chip` and end the VM
    /// through `events`.
    pub(crate) fn new(chip: Arc<dyn IrqChip>, events: EventSender) -> Self {
        Self {
            chip: Arc::new(GatedIrqChip {
                inner: chip,
                armed: AtomicBool::new(false),
            }),
            events,
            buses: Buses::default(),
            lines: BTreeMap::new(),
            devices: Vec::new(),
            conflicts: Vec::new(),
        }
    }

    /// Reserves a window the host serves in the kernel, such as KVM's PIT
    /// ports or the local APIC page, so no device can claim it.
    pub(crate) fn reserve(&mut self, bus: BusKind, base: u64, len: u64, owner: &'static str) {
        self.register(
            Claim {
                bus,
                base,
                len,
                offset: 0,
            },
            owner,
            None,
        );
    }

    /// Adds the device `ctor` constructs, under `name`, with the resources
    /// it claims. The constructor only claims resources and builds state: a
    /// device that needs a thread starts it after [`build`](Self::build).
    /// Fails only when the constructor does; conflicts wait for `build`.
    pub(crate) fn add<D, F>(&mut self, name: &'static str, ctor: F) -> Result<()>
    where
        D: BusDevice + 'static,
        F: FnOnce(&mut Resources) -> Result<D>,
    {
        let mut resources = Resources {
            chip: self.chip.clone(),
            events: self.events.clone(),
            claims: Vec::new(),
            lines: Vec::new(),
        };
        let device: Arc<Mutex<dyn BusDevice>> = Arc::new(Mutex::new(ctor(&mut resources)?));
        for claim in resources.claims {
            let offset = claim.offset;
            self.register(claim, name, Some((Arc::clone(&device), offset)));
        }
        for line in resources.lines {
            // A rejected claim leaves the line with its first holder.
            match self.lines.entry(line) {
                Entry::Vacant(slot) => {
                    slot.insert(name);
                }
                Entry::Occupied(holder) => self.conflicts.push(Conflict::Line {
                    line,
                    device: name,
                    owner: holder.get(),
                }),
            }
        }
        self.devices.push(device);
        Ok(())
    }

    /// The chipset, with every line armed, or every conflict the claims
    /// produced; the devices then drop without having run.
    pub(crate) fn build(self) -> Result<Chipset> {
        if !self.conflicts.is_empty() {
            return Err(Error::Chipset(self.conflicts));
        }
        self.chip.armed.store(true, Ordering::Release);
        Ok(Chipset {
            buses: self.buses,
            _devices: self.devices,
        })
    }

    fn register(&mut self, claim: Claim, claimant: &'static str, target: Option<Target>) {
        let Some(range) = BusRange::new(claim.base, claim.len) else {
            self.conflicts.push(Conflict::InvalidRange {
                bus: claim.bus,
                device: claimant,
                base: claim.base,
                len: claim.len,
            });
            return;
        };
        let bus = match claim.bus {
            BusKind::Mmio => &mut self.buses.mmio,
            #[cfg(target_arch = "x86_64")]
            BusKind::Pio => &mut self.buses.pio,
        };
        if let Err(owner) = bus.insert(range, claimant, target) {
            self.conflicts.push(Conflict::Range {
                bus: claim.bus,
                device: claimant,
                base: claim.base,
                len: claim.len,
                owner,
            });
        }
    }
}

/// The built machine: its buses and the devices behind them. Every vCPU
/// thread borrows it for the run.
pub(crate) struct Chipset {
    buses: Buses,
    /// Every device, kept alive for the chipset's lifetime; the buses hold
    /// the same references.
    _devices: Vec<Arc<Mutex<dyn BusDevice>>>,
}

impl Chipset {
    /// Serves an MMIO or port access and returns `None`; returns every other
    /// exit to the caller unchanged.
    pub(crate) fn dispatch<'a>(&self, exit: VcpuExit<'a>) -> Option<VcpuExit<'a>> {
        self.buses.dispatch(exit)
    }

    /// The buses, whose counters show the accesses no device claimed.
    pub(crate) fn buses(&self) -> &Buses {
        &self.buses
    }
}

#[cfg(test)]
#[path = "../tests/chipset/mod.rs"]
mod tests;
