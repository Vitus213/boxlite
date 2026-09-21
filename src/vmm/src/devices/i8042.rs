// Copyright 2026 BoxLite Contributors
// SPDX-License-Identifier: Apache-2.0

//! The i8042 keyboard controller at ports `0x60` (data) and `0x64` (command).
//!
//! In a microVM this device exists for one instruction: the guest writing
//! `0xFE` (reset CPU) to the command port. BoxLite's guest agent ends every
//! box with `reboot(RESTART)` — x86_64 has no ACPI power-off — so that byte
//! is the VM's shutdown path: the design document has the VMM's i8042 turn
//! the reset `IoOut` into the VM's termination.
//!
//! The reset is reported by setting an injected `Arc<AtomicBool>`; the
//! future `Vm::run` poller watches that flag and drives the stop protocol
//! (design, "Stopping"). The device never exits the process itself.
//!
//! Everything else the driver probes must answer, or kernel probing stalls:
//! a status register whose bits track the controller buffers, the control
//! register commands, and an ACK (`0xFA`) for anything sent to the
//! keyboard. Behavior cross-checked against libkrun/Firecracker's
//! `devices/legacy/i8042` (e12b9b3 / 68698ad), including their five-port
//! window registration at `0x60` that carries the two ports the device
//! separates by offset.

use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::bus::BusDevice;

/// Output-buffer-full: the guest must read port 0x60 before more responses.
const STATUS_OUT_DATA: u8 = 0x01;
/// Input-buffer-full: the controller waits for a parameter byte on 0x60.
const STATUS_IN_DATA: u8 = 0x02;
/// Self-test passed: the BIOS post always ran fine in a VM.
const STATUS_SELF_TEST_OK: u8 = 0x04;

// Controller commands written to port 0x64.
const CMD_READ_CONTROL: u8 = 0x20;
const CMD_WRITE_CONTROL: u8 = 0x60;
const CMD_DISABLE_FIRST_PORT: u8 = 0xAD;
const CMD_ENABLE_FIRST_PORT: u8 = 0xAE;
const CMD_DISABLE_SECOND_PORT: u8 = 0xA7;
const CMD_ENABLE_SECOND_PORT: u8 = 0xA8;
const CMD_READ_OUTPUT_PORT: u8 = 0xD0;
const CMD_WRITE_OUTPUT_PORT: u8 = 0xD1;
const CMD_SET_LED_FOLLOWED_BY_DATA: u8 = 0xED;
const CMD_SELF_TEST: u8 = 0xAA;
const CMD_PULSE_TEST: u8 = 0xEE;
const CMD_READ_ID: u8 = 0xF2;
const CMD_SET_DEFAULTS: u8 = 0xF0;
const CMD_RESET_CPU: u8 = 0xFE;
const CMD_RETEST: u8 = 0xFF;

// Control register bits this emulation keeps.
const CONTROL_FIRST_PORT_ENABLED: u8 = 0x10;
const CONTROL_SECOND_PORT_ENABLED: u8 = 0x20;

/// Commands whose next byte arrives on the data port.
#[derive(PartialEq, Eq)]
enum Expect {
    /// 0xED: the payload sets the keyboard LEDs; we have none.
    SetLed,
    /// 0x60: the payload is the controller's new control register.
    ControlByte,
    /// 0xD1: the payload drives the output port latch.
    OutputPort,
}

/// An emulated i8042 PS/2 host controller.
///
/// Register it on the [`IoBus`] as one five-port window at `0x60` (libkrun's
/// shape): offset 0 is the data port, offset 4 the command/status port.
pub struct I8042 {
    reset: Arc<AtomicBool>,
    /// The controller's control register, readable via 0x20.
    command: u8,
    /// The output latch, readable via 0xD0.
    output_port: u8,
    expect: Option<Expect>,
    /// Controller-to-host responses, drained one byte per 0x60 read.
    resp: VecDeque<u8>,
}

impl I8042 {
    /// The first port of the five-port window the device occupies.
    pub const PORT_BASE: u16 = 0x60;
    /// The width of that window (0x60..=0x64).
    pub const PORT_WINDOW: u16 = 5;

    /// Creates a controller whose CPU-reset command raises `reset_requested`.
    pub fn new(reset_requested: Arc<AtomicBool>) -> Self {
        Self {
            reset: reset_requested,
            // POST passed, both ports enabled: what a clean-booting PC reports.
            command: CONTROL_FIRST_PORT_ENABLED | CONTROL_SECOND_PORT_ENABLED,
            output_port: 0,
            expect: None,
            resp: VecDeque::new(),
        }
    }

    /// Queues a reply, replacing anything the driver never read.
    fn push(&mut self, bytes: &[u8]) {
        self.resp.clear();
        self.resp.extend(bytes);
    }

    fn read_status(&mut self) -> u8 {
        let mut status = STATUS_SELF_TEST_OK;
        if !self.resp.is_empty() {
            status |= STATUS_OUT_DATA;
        }
        if self.expect.is_some() {
            status |= STATUS_IN_DATA;
        }
        status
    }

    fn read_data(&mut self) -> u8 {
        self.resp.pop_front().unwrap_or(0)
    }

    fn write_command(&mut self, value: u8) {
        match value {
            CMD_RESET_CPU => self.reset.store(true, Ordering::SeqCst),
            CMD_READ_CONTROL => self.push(&[self.command]),
            CMD_WRITE_CONTROL => self.expect = Some(Expect::ControlByte),
            // The enable/disable commands fold into the two port-enable
            // bits we keep of the control register.
            CMD_DISABLE_FIRST_PORT => self.command &= !CONTROL_FIRST_PORT_ENABLED,
            CMD_ENABLE_FIRST_PORT => self.command |= CONTROL_FIRST_PORT_ENABLED,
            CMD_DISABLE_SECOND_PORT => self.command &= !CONTROL_SECOND_PORT_ENABLED,
            CMD_ENABLE_SECOND_PORT => self.command |= CONTROL_SECOND_PORT_ENABLED,
            CMD_SELF_TEST => self.push(&[0x55]), // the pass code the probe awaits
            CMD_READ_OUTPUT_PORT => self.push(&[self.output_port]),
            CMD_WRITE_OUTPUT_PORT => self.expect = Some(Expect::OutputPort),
            CMD_SET_LED_FOLLOWED_BY_DATA => self.expect = Some(Expect::SetLed),
            CMD_PULSE_TEST => self.push(&[CMD_PULSE_TEST]), // echo: loopback OK
            // The attached device is a standard scancode-set-2 keyboard.
            CMD_READ_ID => self.push(&[0xFA, 0xAB, 0x00]),
            CMD_SET_DEFAULTS | CMD_RETEST => self.push(&[0xFA, 0x00]),
            _ => self.push(&[0xFA]),
        }
    }

    fn write_data(&mut self, value: u8) {
        match self.expect.take() {
            Some(Expect::ControlByte) => self.command = value,
            Some(Expect::OutputPort) => self.output_port = value,
            // LEDs: there is no physical keyboard, so swallow the parameter.
            Some(Expect::SetLed) => {}
            // An unrequested byte on the data port is a command to the
            // keyboard device; blindly ACK it, as libkrun's emulation does.
            None => self.push(&[0xFA]),
        }
    }
}

impl BusDevice for I8042 {
    /// Offset 0 serves the data port and offset 4 the command/status port;
    /// a multi-byte access visits distinct ports byte by byte, like
    /// consecutive `inb`/`outb` across the five-port window. Only reads of
    /// the data port pop the reply queue. Ports between and outside read 0
    /// and ignore writes.
    fn read(&mut self, offset: u64, data: &mut [u8]) {
        for (i, byte) in data.iter_mut().enumerate() {
            *byte = match offset.saturating_add(i as u64) {
                0 => self.read_data(),
                4 => self.read_status(),
                _ => 0,
            };
        }
    }

    fn write(&mut self, offset: u64, data: &[u8]) {
        for (i, byte) in data.iter().enumerate() {
            match offset.saturating_add(i as u64) {
                0 => self.write_data(*byte),
                4 => self.write_command(*byte),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn controller() -> (I8042, Arc<AtomicBool>) {
        let reset = Arc::new(AtomicBool::new(false));
        (I8042::new(Arc::clone(&reset)), reset)
    }

    /// Reads the data port (window offset 0).
    fn read_data(dev: &mut I8042) -> u8 {
        let mut data = [0u8; 1];
        dev.read(0, &mut data);
        data[0]
    }

    /// Reads the status port (window offset 4).
    fn read_status(dev: &mut I8042) -> u8 {
        let mut data = [0u8; 1];
        dev.read(4, &mut data);
        data[0]
    }

    fn write_command(dev: &mut I8042, value: u8) {
        dev.write(4, &[value]);
    }

    fn write_data(dev: &mut I8042, value: u8) {
        dev.write(0, &[value]);
    }

    #[test]
    fn reset_command_sets_the_shutdown_flag() {
        let (mut dev, reset) = controller();
        assert!(!reset.load(Ordering::SeqCst));
        write_command(&mut dev, CMD_RESET_CPU);
        assert!(reset.load(Ordering::SeqCst));
    }

    #[test]
    fn status_tracks_the_output_buffer() {
        let (mut dev, _) = controller();
        // Fresh controller: self-test OK, no pending byte.
        assert_eq!(read_status(&mut dev), STATUS_SELF_TEST_OK);
        write_command(&mut dev, CMD_READ_ID);
        assert_eq!(read_status(&mut dev) & STATUS_OUT_DATA, STATUS_OUT_DATA);
        assert_eq!(read_data(&mut dev), 0xFA);
        assert_eq!(read_data(&mut dev), 0xAB);
        assert_eq!(read_data(&mut dev), 0x00);
        // Drained: the data-ready bit drops again.
        assert_eq!(read_status(&mut dev) & STATUS_OUT_DATA, 0);
        // Reading past the end answers zero, never a panic.
        assert_eq!(read_data(&mut dev), 0);
    }

    #[test]
    fn port_enables_fold_into_the_control_register() {
        let (mut dev, _) = controller();
        write_command(&mut dev, CMD_DISABLE_FIRST_PORT);
        write_command(&mut dev, CMD_DISABLE_SECOND_PORT);
        write_command(&mut dev, CMD_READ_CONTROL);
        assert_eq!(read_data(&mut dev), 0);
        write_command(&mut dev, CMD_ENABLE_FIRST_PORT);
        write_command(&mut dev, CMD_READ_CONTROL);
        assert_eq!(read_data(&mut dev), CONTROL_FIRST_PORT_ENABLED);
        write_command(&mut dev, CMD_ENABLE_SECOND_PORT);
        write_command(&mut dev, CMD_READ_CONTROL);
        assert_eq!(
            read_data(&mut dev),
            CONTROL_FIRST_PORT_ENABLED | CONTROL_SECOND_PORT_ENABLED
        );
    }

    #[test]
    fn write_control_is_a_two_step_command() {
        let (mut dev, _) = controller();
        write_command(&mut dev, CMD_WRITE_CONTROL);
        // Status shows the controller waiting for a parameter byte.
        assert_eq!(read_status(&mut dev) & STATUS_IN_DATA, STATUS_IN_DATA);
        write_data(&mut dev, 0x43);
        assert_eq!(read_status(&mut dev) & STATUS_IN_DATA, 0);
        write_command(&mut dev, CMD_READ_CONTROL);
        assert_eq!(read_data(&mut dev), 0x43);
    }

    #[test]
    fn led_command_consumes_its_parameter_silently() {
        let (mut dev, _) = controller();
        write_command(&mut dev, CMD_SET_LED_FOLLOWED_BY_DATA);
        assert_eq!(read_status(&mut dev) & STATUS_IN_DATA, STATUS_IN_DATA);
        write_data(&mut dev, 0x01);
        assert_eq!(read_status(&mut dev) & STATUS_IN_DATA, 0); // parameter consumed
        assert_eq!(read_status(&mut dev) & STATUS_OUT_DATA, 0); // no reply queued
    }

    #[test]
    fn echo_and_default_replies() {
        let (mut dev, _) = controller();
        write_command(&mut dev, CMD_PULSE_TEST);
        assert_eq!(read_data(&mut dev), 0xEE);
        write_command(&mut dev, CMD_SET_DEFAULTS);
        assert_eq!(read_data(&mut dev), 0xFA);
        assert_eq!(read_data(&mut dev), 0x00);
        write_command(&mut dev, CMD_RETEST);
        assert_eq!(read_data(&mut dev), 0xFA);
        assert_eq!(read_data(&mut dev), 0x00);
    }

    #[test]
    fn unknown_command_and_bare_keyboard_writes_always_ack() {
        let (mut dev, _) = controller();
        write_command(&mut dev, 0xF1); // unsupported controller command
        assert_eq!(read_data(&mut dev), 0xFA);
        // Direct keyboard command with nothing pending: ACK.
        write_data(&mut dev, 0xFF);
        assert_eq!(read_data(&mut dev), 0xFA);
    }

    #[test]
    fn output_port_round_trips() {
        let (mut dev, _) = controller();
        write_command(&mut dev, CMD_WRITE_OUTPUT_PORT);
        write_data(&mut dev, 0xA2);
        write_command(&mut dev, CMD_READ_OUTPUT_PORT);
        assert_eq!(read_data(&mut dev), 0xA2);
    }

    #[test]
    fn wide_access_walks_ports_without_panic() {
        let (mut dev, _) = controller();
        write_command(&mut dev, CMD_READ_ID);
        // A wide access visits distinct ports: only offset 0 pops the data
        // queue, offsets 1..3 are unclaimed, offset 4 is the status port.
        let mut data = [0u8; 5];
        dev.read(0, &mut data);
        assert_eq!(data[0], 0xFA);
        assert_eq!(data[1..4], [0, 0, 0]);
        assert_eq!(data[4] & STATUS_OUT_DATA, STATUS_OUT_DATA); // AB, 00 queued
        assert_eq!(read_data(&mut dev), 0xAB);
        assert_eq!(read_data(&mut dev), 0x00);
        // A write walk that crosses both ports: ACK replies, never a panic.
        dev.write(0, &[0x00; 8]);
        assert_eq!(read_status(&mut dev) & STATUS_OUT_DATA, STATUS_OUT_DATA);
    }
}
