// Copyright 2026 BoxLite Contributors
// SPDX-License-Identifier: Apache-2.0

//! Window registration and dispatch rules of the MMIO and port buses.

use std::sync::PoisonError;

use super::*;

/// Echoes `offset + index` on reads so routing tests can identify the window.
struct Echo;

impl BusDevice for Echo {
    fn read(&mut self, offset: u64, data: &mut [u8]) {
        for (i, byte) in data.iter_mut().enumerate() {
            *byte = offset as u8 + i as u8;
        }
    }

    fn write(&mut self, _offset: u64, _data: &[u8]) {}
}

fn device() -> Arc<Mutex<dyn BusDevice + Send>> {
    Arc::new(Mutex::new(Echo))
}

#[test]
fn insert_rejects_invalid_windows() {
    let mut bus = Bus::new();
    assert!(matches!(
        bus.insert(0x1000, 0, device()),
        Err(Error::InvalidWindow {
            base: 0x1000,
            size: 0
        })
    ));
    // A window that runs past the end of the address space would defeat
    // exact overlap and containment arithmetic: rejected at the boundary.
    assert!(matches!(
        bus.insert(u64::MAX, 2, device()),
        Err(Error::InvalidWindow { .. })
    ));
    // And a re-registration at the same base is still an Overlap, not a
    // silent replacement.
    bus.insert(0x2000, 0x1000, device()).unwrap();
    assert!(matches!(
        bus.insert(0x2000, 0x1000, device()),
        Err(Error::Overlap { .. })
    ));
}

#[test]
fn insert_allows_adjacent_windows() {
    let mut bus = Bus::new();
    bus.insert(0x1000, 2, device()).unwrap();
    bus.insert(0x1002, 2, device()).unwrap();
}

#[test]
fn insert_rejects_overlapping_windows() {
    let mut bus = Bus::new();
    bus.insert(0x1000, 2, device()).unwrap();
    bus.insert(0x1002, 2, device()).unwrap(); // adjacent: allowed
    assert!(matches!(
        bus.insert(0x1003, 2, device()),
        Err(Error::Overlap { .. })
    ));
    assert!(matches!(
        bus.insert(0x0FF0, 0x20, device()),
        Err(Error::Overlap { .. })
    ));
}

#[test]
fn overlap_display_names_the_range() {
    let error = Error::Overlap {
        base: 0xd000_0000,
        size: 0x1000,
    };
    assert_eq!(
        error.to_string(),
        "device at 0xd0000000+4096 overlaps an existing device"
    );
}

#[test]
fn read_write_route_by_address_with_device_offsets() {
    let mut bus = Bus::new();
    bus.insert(0x2000, 0x100, device()).unwrap();
    bus.insert(0x4000, 0x100, device()).unwrap();

    let mut data = [0u8; 4];
    bus.read(0x4010, &mut data).unwrap();
    assert_eq!(data, [0x10, 0x11, 0x12, 0x13]);

    bus.write(0x20f8, &[1, 2, 3, 4]).unwrap();
}

#[test]
fn read_one_byte_past_the_window_end_is_unmapped() {
    let mut bus = Bus::new();
    bus.insert(0x5000, 4, device()).unwrap();
    let mut data = [0u8; 1];
    assert!(matches!(
        bus.read(0x5004, &mut data),
        Err(Error::Unmapped { addr: 0x5004 })
    ));
}

#[test]
fn read_wider_than_the_window_is_unmapped() {
    let mut bus = Bus::new();
    bus.insert(0x6000, 4, device()).unwrap();
    // Starts inside, would spill past the end: no split-access path exists.
    let mut data = [0u8; 4];
    assert!(matches!(
        bus.read(0x6002, &mut data),
        Err(Error::Unmapped { addr: 0x6002 })
    ));
}

#[test]
fn unmapped_display_names_the_address() {
    let error = Error::Unmapped { addr: 0xdead_beef };
    assert_eq!(error.to_string(), "no device at guest address 0xdeadbeef");
}

#[test]
fn write_to_unmapped_reports_unmapped() {
    let bus = Bus::new();
    assert!(matches!(
        bus.write(0x9000, &[1]),
        Err(Error::Unmapped { addr: 0x9000 })
    ));
}

#[test]
fn poisoned_device_reports_instead_of_panic() {
    let mut bus = Bus::new();
    let shared: Arc<Mutex<dyn BusDevice + Send>> = device();
    bus.insert(0x7000, 8, Arc::clone(&shared)).unwrap();

    let poisoner = Arc::clone(&shared);
    let handle = std::thread::spawn(move || {
        let _guard = lock_ok(&poisoner);
        panic!("poison the device lock");
    });
    assert!(handle.join().is_err());

    let mut data = [0u8; 1];
    assert!(matches!(
        bus.read(0x7000, &mut data),
        Err(Error::DevicePoisoned { addr: 0x7000 })
    ));
    assert!(matches!(
        bus.write(0x7000, &[0]),
        Err(Error::DevicePoisoned { addr: 0x7000 })
    ));
}

/// Locks without panicking: the poisoning test holds this guard when it panics.
fn lock_ok<T: BusDevice + Send + ?Sized>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[test]
fn poisoned_display_names_the_address() {
    let error = Error::DevicePoisoned { addr: 0x7000 };
    assert_eq!(error.to_string(), "device at 0x7000 is poisoned");
}

#[cfg(target_arch = "x86_64")]
#[test]
fn io_bus_routes_and_rejects_overlap() {
    let mut bus = IoBus::new();
    bus.insert(0x3F8, 8, device()).unwrap();
    assert!(matches!(
        bus.insert(0x3FD, 8, device()),
        Err(Error::IoOverlap { port: 0x3FD, .. })
    ));
    bus.insert(0x400, 8, device()).unwrap(); // adjacent to 0x3F8+8

    // A 1-byte read at 0x3FD lands at offset 5 of the first window.
    let mut data = [0u8; 1];
    bus.read(0x3FD, &mut data).unwrap();
    assert_eq!(data, [5]);
    bus.write(0x403, &[1]).unwrap();
}

#[cfg(target_arch = "x86_64")]
#[test]
fn io_bus_reports_unmapped_ports() {
    let mut bus = IoBus::new();
    bus.insert(0x3F8, 8, device()).unwrap();
    // 0x3FF is the last mapped port; 0x400 belongs to nobody.
    let mut data = [0u8; 1];
    bus.read(0x3FF, &mut data).unwrap();
    assert_eq!(data, [7]);
    assert!(matches!(
        bus.read(0x3F7, &mut data),
        Err(Error::IoUnmapped { port: 0x3F7 })
    ));
    assert!(matches!(
        bus.read(0x400, &mut data),
        Err(Error::IoUnmapped { port: 0x400 })
    ));
    assert!(matches!(
        bus.write(0x400, &[0; 4]),
        Err(Error::IoUnmapped { port: 0x400 })
    ));
    // A four-byte access from 0x3FC spills the window: unmapped.
    let mut wide = [0u8; 4];
    assert!(matches!(
        bus.read(0x3FD, &mut wide),
        Err(Error::IoUnmapped { port: 0x3FD })
    ));

    assert_eq!(
        Error::IoOverlap {
            port: 0x60,
            size: 1
        }
        .to_string(),
        "device at port 0x60+1 overlaps an existing device"
    );
    assert_eq!(
        Error::IoUnmapped { port: 0x63 }.to_string(),
        "no device at port 0x63"
    );
}
