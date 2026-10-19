// Copyright 2026 BoxLite Contributors
// SPDX-License-Identifier: Apache-2.0

//! Address-range registration and device I/O dispatch.
//!
//! Guest accesses to RAM never leave the hypervisor, so the VMM only sees
//! exits for device windows. A [`Bus`] maps those windows to [`BusDevice`]
//! implementations, and a handled `MmioRead`/`MmioWrite` (or, on x86_64, an
//! `IoIn`/`IoOut` through [`IoBus`]) is resolved here: find the window,
//! subtract its base, and call the device with the offset.
//!
//! A vCPU thread holds a device's lock for exactly one register access (see
//! the design document's locking rule in `docs/contributing/architecture/vmm/README.md`).
//! The M1 legacy devices (8250, CMOS RTC, i8042) have host-side work that
//! cannot block — writing to a sink, reading the system clock, setting an
//! atomic flag — so they are serviced inline and own no worker thread.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use crate::error::{Error, Result};

/// The minimal face a simulated device shows to a bus: one register access.
///
/// `offset` is measured from the device's registered base address, so a
/// device never learns where it sits in the guest address space. `data`
/// carries the access: its length is the access width (1, 2, 4, or 8 bytes
/// on real hardware), and `read` fills it with the device's response.
pub trait BusDevice: Send {
    /// Reads `data.len()` bytes starting at `offset` into `data`.
    fn read(&mut self, offset: u64, data: &mut [u8]);

    /// Writes `data` starting at `offset`.
    fn write(&mut self, offset: u64, data: &[u8]);
}

/// A registered device plus the size of its window.
type Window = (u64, Arc<Mutex<dyn BusDevice + Send>>);

/// Returns whether `[start, start + len)` overlaps `[other_start, other_start + other_len)`.
///
/// Both ends are exact: `insert` rejects any window whose range would
/// overflow the address space, so every stored end — and every start or
/// size a caller may pass in under that same rule — is representable and
/// plain addition never wraps. Adjacency (one range ending where the other
/// starts) is not an overlap.
fn overlaps(start: u64, len: u64, other_start: u64, other_len: u64) -> bool {
    start < other_start + other_len && other_start < start + len
}

/// The MMIO bus: guest physical address ranges mapped to devices.
///
/// Windows are registered while the VM is built and never overlap; the
/// access path only reads the map, so it is usable from every vCPU thread
/// without further synchronization (device state stays behind each
/// window's `Mutex`).
#[derive(Default)]
pub struct Bus {
    /// key: window base; value: (size, device)
    ranges: BTreeMap<u64, Window>,
}

impl Bus {
    /// Creates an empty bus.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `device` for `[base, base + size)`.
    ///
    /// Fails with [`Error::InvalidWindow`] if `size` is zero or the range
    /// would run past the end of the address space, and with
    /// [`Error::Overlap`] if the range touches a registered window.
    pub fn insert(
        &mut self,
        base: u64,
        size: u64,
        device: Arc<Mutex<dyn BusDevice + Send>>,
    ) -> Result<()> {
        if size == 0 || base.checked_add(size).is_none() {
            return Err(Error::InvalidWindow { base, size });
        }
        if self
            .ranges
            .iter()
            .any(|(old_base, (old_size, _))| overlaps(base, size, *old_base, *old_size))
        {
            return Err(Error::Overlap { base, size });
        }
        self.ranges.insert(base, (size, device));
        Ok(())
    }

    /// Dispatches a guest read at `addr` to its device.
    ///
    /// The whole access must land in one window, as it would on real
    /// hardware; a split or unmapped access fails with [`Error::Unmapped`]
    /// and the caller reports a fatal guest exit rather than stitching two
    /// devices together.
    pub fn read(&self, addr: u64, data: &mut [u8]) -> Result<()> {
        let (base, device) = self.window(addr, data.len() as u64)?;
        let mut guard = device.lock().map_err(|_| Error::DevicePoisoned { addr })?;
        guard.read(addr - base, data);
        Ok(())
    }

    /// Dispatches a guest write at `addr` to its device. See [`Bus::read`]
    /// for the one-window rule.
    pub fn write(&self, addr: u64, data: &[u8]) -> Result<()> {
        let (base, device) = self.window(addr, data.len() as u64)?;
        let mut guard = device.lock().map_err(|_| Error::DevicePoisoned { addr })?;
        guard.write(addr - base, data);
        Ok(())
    }

    /// Finds the window whose range fully contains `[addr, addr + len)`.
    ///
    /// Windows never overlap, so the greatest base at or below `addr` is
    /// the only candidate.
    fn window(&self, addr: u64, len: u64) -> Result<(u64, &Arc<Mutex<dyn BusDevice + Send>>)> {
        let end = addr.checked_add(len).ok_or(Error::Unmapped { addr })?;
        let (base, (size, device)) = self
            .ranges
            .range(..=addr)
            .next_back()
            .ok_or(Error::Unmapped { addr })?;
        if end <= base + size {
            Ok((*base, device))
        } else {
            Err(Error::Unmapped { addr })
        }
    }
}

/// The x86_64 port I/O bus: `in`/`out` ports mapped to devices.
///
/// Same dispatch rules as [`Bus`], over the separate 16-bit port space that
/// the legacy PC devices (serial, RTC, keyboard controller) occupy. It is a
/// distinct type rather than a generic one because ports are `u16` and fail
/// with port-specific diagnostics.
#[cfg(target_arch = "x86_64")]
#[derive(Default)]
pub struct IoBus {
    /// key: window base port; value: (size, device)
    ranges: BTreeMap<u16, Window>,
}

#[cfg(target_arch = "x86_64")]
impl IoBus {
    /// Creates an empty port bus.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `device` for ports `[base, base + size)`.
    ///
    /// Fails with [`Error::InvalidWindow`] on a zero size or a range that
    /// runs past the end of the 16-bit port space, and with
    /// [`Error::IoOverlap`] if the range touches a registered window;
    /// adjacency is allowed, as on [`Bus`]. One device may own several
    /// windows by registering the same `Arc` once per window, which is how
    /// the i8042 spans non-adjacent ports.
    pub fn insert(
        &mut self,
        base: u16,
        size: u16,
        device: Arc<Mutex<dyn BusDevice + Send>>,
    ) -> Result<()> {
        let wide = u64::from(size);
        if size == 0 || u64::from(base) + wide > 0x1_0000 {
            return Err(Error::InvalidWindow {
                base: u64::from(base),
                size: wide,
            });
        }
        if self.ranges.iter().any(|(old_base, (old_size, _))| {
            overlaps(u64::from(base), wide, u64::from(*old_base), *old_size)
        }) {
            return Err(Error::IoOverlap { port: base, size });
        }
        self.ranges.insert(base, (wide, device));
        Ok(())
    }
    /// Dispatches a guest read from `port` to its device.
    pub fn read(&self, port: u16, data: &mut [u8]) -> Result<()> {
        let (base, device) = self.window(port, data.len() as u64)?;
        let mut guard = device.lock().map_err(|_| Error::DevicePoisoned {
            addr: u64::from(port),
        })?;
        guard.read(u64::from(port) - base, data);
        Ok(())
    }

    /// Dispatches a guest write to `port` to its device.
    pub fn write(&self, port: u16, data: &[u8]) -> Result<()> {
        let (base, device) = self.window(port, data.len() as u64)?;
        let mut guard = device.lock().map_err(|_| Error::DevicePoisoned {
            addr: u64::from(port),
        })?;
        guard.write(u64::from(port) - base, data);
        Ok(())
    }

    /// Finds the window whose ports fully contain `[port, port + len)`.
    ///
    /// As on [`Bus::window`], the greatest base at or below `port` is the
    /// only candidate because windows never overlap. The end sum is
    /// checked, like [`Bus::window`]: `insert` confines stored ends to
    /// the 16-bit port space, and a checked add keeps a hostile or
    /// absurdly wide access from wrapping.
    fn window(&self, port: u16, len: u64) -> Result<(u64, &Arc<Mutex<dyn BusDevice + Send>>)> {
        let end = u64::from(port)
            .checked_add(len)
            .ok_or(Error::IoUnmapped { port })?;
        let (base, (size, device)) = self
            .ranges
            .range(..=port)
            .next_back()
            .ok_or(Error::IoUnmapped { port })?;
        if end <= u64::from(*base) + *size {
            Ok((u64::from(*base), device))
        } else {
            Err(Error::IoUnmapped { port })
        }
    }
}

// The suite lives under `tests/` so it stays out of the counted source; it is
// still a unit-test module with access to crate-private items.
#[cfg(test)]
#[path = "../tests/bus/mod.rs"]
mod tests;
