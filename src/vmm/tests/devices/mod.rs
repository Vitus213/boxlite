// Copyright 2026 BoxLite Contributors
// SPDX-License-Identifier: Apache-2.0

//! The serial and i8042 adapters, driven through the port bus the way a
//! guest kernel reaches them: no /dev/kvm and no vCPU.

use std::{
    io::{self, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use super::{i8042::I8042, serial::Serial};
use crate::{
    bus::IoBus,
    error::Error,
    irq::{InterruptTarget, IrqSender},
};

/// Records every interrupt-line transition.
#[derive(Default)]
struct RecordingTarget {
    calls: Mutex<Vec<(u32, bool)>>,
}

impl RecordingTarget {
    fn take(&self) -> Vec<(u32, bool)> {
        std::mem::take(&mut *self.calls.lock().unwrap())
    }
}

impl InterruptTarget for RecordingTarget {
    fn set_irq_line(&self, line: u32, level: bool) -> boxlite_hypervisor::Result<()> {
        self.calls.lock().unwrap().push((line, level));
        Ok(())
    }
}

/// Console bytes the test can inspect.
#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Vec<u8>>>);

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A sink whose host side has gone away.
struct ClosedSink;

impl Write for ClosedSink {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(io::ErrorKind::BrokenPipe.into())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The two devices registered on a port bus exactly as M1 builds them.
struct Machine {
    bus: IoBus,
    serial: Arc<Mutex<Serial>>,
    sink: Sink,
    irq: Arc<RecordingTarget>,
    reset: Arc<AtomicBool>,
}

fn machine() -> Machine {
    let sink = Sink::default();
    let irq = Arc::new(RecordingTarget::default());
    let target: Arc<dyn InterruptTarget> = irq.clone();
    let serial = Arc::new(Mutex::new(Serial::new(
        Box::new(sink.clone()),
        IrqSender::new(target),
    )));
    let reset = Arc::new(AtomicBool::new(false));

    let mut bus = IoBus::new();
    bus.insert(Serial::PORT_BASE, Serial::PORT_WINDOW, serial.clone())
        .unwrap();
    bus.insert(
        I8042::PORT_BASE,
        I8042::PORT_WINDOW,
        Arc::new(Mutex::new(I8042::new(Arc::clone(&reset)))),
    )
    .unwrap();

    Machine {
        bus,
        serial,
        sink,
        irq,
        reset,
    }
}

impl Machine {
    fn read(&self, port: u16) -> u8 {
        let mut data = [0u8; 1];
        self.bus.read(port, &mut data).unwrap();
        data[0]
    }

    fn write(&self, port: u16, value: u8) {
        self.bus.write(port, &[value]).unwrap();
    }
}

const COM1: u16 = Serial::PORT_BASE;
const KBD_COMMAND: u16 = I8042::PORT_BASE + 4;

#[test]
fn console_init_and_output_reach_the_sink_without_a_break() {
    let m = machine();

    // Idle LSR: transmitter empty, no data, and no break bit (0x10).
    assert_eq!(m.read(COM1 + 5), 0x60);

    // 16550A init: DLAB, divisor 12, 8N1, FIFO on, then read back.
    m.write(COM1 + 3, 0x80);
    m.write(COM1, 0x0C);
    m.write(COM1 + 1, 0x00);
    assert_eq!(m.read(COM1), 0x0C);
    m.write(COM1 + 3, 0x03);
    m.write(COM1 + 2, 0x07);
    assert_eq!(m.read(COM1 + 3), 0x03);
    assert_eq!(m.read(COM1 + 2) & 0xC0, 0xC0, "IIR reports a 16550A FIFO");

    for byte in b"hello" {
        m.write(COM1, *byte);
    }
    assert_eq!(*m.sink.0.lock().unwrap(), b"hello");
    assert!(m.irq.take().is_empty(), "THR interrupt is disabled");
}

#[test]
fn received_input_pulses_gsi_4_and_drains_through_rbr() {
    let m = machine();
    m.write(COM1 + 1, 0x01); // enable received-data interrupts

    assert_eq!(m.serial.lock().unwrap().enqueue_input(b"x"), 1);
    assert_eq!(
        m.irq.take(),
        vec![(Serial::GSI, true), (Serial::GSI, false)]
    );
    assert_eq!(m.read(COM1 + 5) & 0x01, 1, "data ready");
    assert_eq!(m.read(COM1), b'x');
    assert_eq!(m.read(COM1 + 5) & 0x01, 0);
}

#[test]
fn a_closed_sink_drops_output_without_stalling_the_guest() {
    let irq: Arc<dyn InterruptTarget> = Arc::new(RecordingTarget::default());
    let mut bus = IoBus::new();
    let serial = Serial::new(Box::new(ClosedSink), IrqSender::new(irq));
    bus.insert(COM1, Serial::PORT_WINDOW, Arc::new(Mutex::new(serial)))
        .unwrap();

    bus.write(COM1, b"x").unwrap();
    let mut lsr = [0u8];
    bus.read(COM1 + 5, &mut lsr).unwrap();
    assert_eq!(lsr[0] & 0x20, 0x20, "transmitter still reads empty");
}

#[test]
fn wide_accesses_walk_registers_and_never_panic() {
    let m = machine();
    let mut data = [0u8; 4];
    m.bus.read(COM1 + 4, &mut data).unwrap();
    // MCR (OUT2 default), LSR idle, MSR (DCD|DSR|CTS), SCR.
    assert_eq!(data, [0x08, 0x60, 0xB0, 0x00]);

    use crate::bus::BusDevice;
    let mut serial = m.serial.lock().unwrap();
    serial.read(u64::MAX - 1, &mut data);
    serial.write(u64::MAX - 1, &data);
}

#[test]
fn only_the_reset_command_on_port_0x64_requests_a_reset() {
    let m = machine();

    // `kb_wait` sees an empty input buffer; other commands do nothing.
    assert_eq!(m.read(KBD_COMMAND), 0);
    m.write(I8042::PORT_BASE, 0xFE);
    m.write(KBD_COMMAND, 0xAA);
    assert!(!m.reset.load(Ordering::SeqCst));

    m.write(KBD_COMMAND, 0xFE);
    assert!(m.reset.load(Ordering::SeqCst));
}

#[test]
fn ports_outside_the_windows_stay_unmapped() {
    let m = machine();
    for port in [0x100u16, 0x65, 0x400] {
        assert!(matches!(
            m.bus.read(port, &mut [0u8; 1]),
            Err(Error::IoUnmapped { port: p }) if p == port
        ));
    }
}
