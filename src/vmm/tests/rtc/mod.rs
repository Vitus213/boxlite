// Copyright 2026 BoxLite Contributors
// SPDX-License-Identifier: Apache-2.0

//! CMOS register rendering, register-B modes and the SET freeze.

use super::*;
use std::time::Duration;

/// 2026-09-22T12:34:56Z (a Tuesday), the fixed wall clock under test.
const NOW: u64 = 1_790_080_496;

fn rtc_at(epoch: u64) -> CmosRtc {
    CmosRtc::new(move || UNIX_EPOCH + Duration::from_secs(epoch))
}

fn read_reg(rtc: &mut CmosRtc, reg: u8) -> u8 {
    let mut out = [0u8; 1];
    rtc.write(0, &[reg]);
    rtc.read(1, &mut out);
    out[0]
}

fn write_reg(rtc: &mut CmosRtc, reg: u8, value: u8) {
    rtc.write(0, &[reg]);
    rtc.write(1, &[value]);
}

#[test]
fn bcd_mode_matches_the_pc_convention() {
    let mut rtc = rtc_at(NOW);
    write_reg(&mut rtc, 0x0B, B_24H); // BCD, 24-hour
    assert_eq!(read_reg(&mut rtc, 0x00), 0x56); // 56 s
    assert_eq!(read_reg(&mut rtc, 0x02), 0x34); // 34 m
    assert_eq!(read_reg(&mut rtc, 0x04), 0x12); // 12 h
    assert_eq!(read_reg(&mut rtc, 0x07), 0x22); // 22nd
    assert_eq!(read_reg(&mut rtc, 0x08), 0x09); // September
    assert_eq!(read_reg(&mut rtc, 0x09), 0x26); // year 2026
    assert_eq!(read_reg(&mut rtc, 0x06), 0x03); // weekday 3 (1 = Sunday)
}

#[test]
fn binary_mode_follows_register_b() {
    let mut rtc = rtc_at(NOW);
    write_reg(&mut rtc, 0x0B, B_DM | B_24H);
    assert_eq!(read_reg(&mut rtc, 0x00), 56);
    assert_eq!(read_reg(&mut rtc, 0x02), 34);
    assert_eq!(read_reg(&mut rtc, 0x04), 12);
    assert_eq!(read_reg(&mut rtc, 0x07), 22);
}

#[test]
fn twelve_hour_mode_packs_pm_into_bit7() {
    // 13:00:00 UTC → 1 PM.
    let mut pm = rtc_at(NOW + 3600 - 34 * 60 - 56);
    write_reg(&mut pm, 0x0B, 0x00); // BCD, 12-hour
    assert_eq!(read_reg(&mut pm, 0x04), 0x01 | 0x80);
    // Midnight reads as 12 AM.
    let mut midnight = rtc_at(NOW - 12 * 3600 - 34 * 60 - 56);
    write_reg(&mut midnight, 0x0B, 0x00);
    assert_eq!(read_reg(&mut midnight, 0x04), 0x12);
    // Noon is 12 PM.
    let mut noon = rtc_at(NOW);
    write_reg(&mut noon, 0x0B, 0x00);
    assert_eq!(read_reg(&mut noon, 0x04), 0x12 | 0x80);
}

#[test]
fn set_bit_freezes_the_clock_to_stored_ram() {
    let mut rtc = rtc_at(NOW);
    write_reg(&mut rtc, 0x0B, B_SET);
    assert_eq!(read_reg(&mut rtc, 0x00), 0x00); // frozen at NVRAM zero
    write_reg(&mut rtc, 0x00, 0x42);
    assert_eq!(read_reg(&mut rtc, 0x00), 0x42);
    write_reg(&mut rtc, 0x0B, B_24H); // leave SET: live again
    assert_eq!(read_reg(&mut rtc, 0x00), 0x56);
}

#[test]
fn alarm_registers_keep_the_dont_care_wildcard() {
    // MC146818: an alarm byte of 0xC0-0xFF matches any value. rtc-cmos
    // writes 0xFF for an unspecified field and reads it back.
    let mut rtc = rtc_at(NOW);
    for reg in [0x01, 0x03, 0x05] {
        write_reg(&mut rtc, reg, 0xFF);
        assert_eq!(read_reg(&mut rtc, reg), 0xFF, "register {reg:#04x}");
    }
    // A programmed alarm survives normal reads, also while SET is held.
    write_reg(&mut rtc, 0x05, 0x12);
    write_reg(&mut rtc, 0x0B, B_SET | B_24H);
    assert_eq!(read_reg(&mut rtc, 0x05), 0x12);
}

#[test]
fn boots_in_24_hour_bcd_mode() {
    // A PC BIOS leaves register B at 0x02. Linux's rtc-cmos refuses to
    // probe ("only 24-hr supported") when bit 1 is clear and the RTC has
    // an IRQ, which it does on x86 without ACPI.
    let mut rtc = rtc_at(NOW + 3600); // 13:34:56 UTC
    assert_eq!(read_reg(&mut rtc, 0x0B), B_24H);
    assert_eq!(read_reg(&mut rtc, 0x04), 0x13);
    assert_eq!(read_reg(&mut rtc, 0x00), 0x56);
}

#[test]
fn status_registers_are_wired_constants() {
    let mut rtc = rtc_at(NOW);
    assert_eq!(read_reg(&mut rtc, 0x0A), REG_A);
    assert_eq!(read_reg(&mut rtc, 0x0C), 0x00);
    assert_eq!(read_reg(&mut rtc, 0x0D), REG_D);
    assert_eq!(read_reg(&mut rtc, 0x0D), 0x80); // VRF set: valid RAM and time
    // Writing them changes nothing observable.
    write_reg(&mut rtc, 0x0A, 0xFF);
    write_reg(&mut rtc, 0x0C, 0xFF);
    write_reg(&mut rtc, 0x0D, 0xFF);
    assert_eq!(read_reg(&mut rtc, 0x0A), REG_A);
    assert_eq!(read_reg(&mut rtc, 0x0C), 0x00);
    assert_eq!(read_reg(&mut rtc, 0x0D), REG_D);
}

#[test]
fn nvram_round_trips_outside_the_live_range() {
    let mut rtc = rtc_at(NOW);
    write_reg(&mut rtc, 0x2A, 0x5A);
    assert_eq!(read_reg(&mut rtc, 0x2A), 0x5A);
    write_reg(&mut rtc, 0x33, 0x20); // NVRAM byte (not a live register)
    assert_eq!(read_reg(&mut rtc, 0x33), 0x20);
    write_reg(&mut rtc, 0x7F, 0x11);
    assert_eq!(read_reg(&mut rtc, 0x7F), 0x11);
}

#[test]
fn index_bit7_nmi_flag_does_not_alias_a_missing_register() {
    let mut rtc = rtc_at(NOW);
    write_reg(&mut rtc, 0x0B, B_24H);
    rtc.write(0, &[0x80 | 0x0B]); // NMI disabled, view of register B
    let mut out = [0u8; 1];
    rtc.read(1, &mut out);
    assert_eq!(out[0], B_24H);
    // A write through the NMI-flagged index still targets register B.
    rtc.write(1, &[0x00]);
    assert_eq!(read_reg(&mut rtc, 0x0B), 0x00);
    // And bit 7 itself reads back on the index port.
    rtc.write(0, &[0x80]);
    let mut out = [0u8; 1];
    rtc.read(0, &mut out);
    assert_eq!(out[0], 0x80);
}

#[test]
fn leap_day_renders_correctly() {
    // 2024-02-29T00:00:00Z.
    let mut rtc = rtc_at(1_709_164_800);
    write_reg(&mut rtc, 0x0B, B_DM | B_24H);
    assert_eq!(read_reg(&mut rtc, 0x07), 29);
    assert_eq!(read_reg(&mut rtc, 0x08), 2);
    assert_eq!(read_reg(&mut rtc, 0x06), 5); // Thursday = 5 (1 = Sunday)
    assert_eq!(read_reg(&mut rtc, 0x09), 24);
}

#[test]
fn century_register_live_renders() {
    // Register 0x32 is the century byte Linux's mc146818_get_time reads
    // to form the full year (0x09 + 0x32*100). It is always binary, so
    // the BCD/binary bit in register B does not apply.
    let mut rtc = rtc_at(NOW); // 2026-09-22
    write_reg(&mut rtc, 0x0B, B_24H); // BCD mode for the other registers
    assert_eq!(read_reg(&mut rtc, 0x32), 20);
    assert_eq!(read_reg(&mut rtc, 0x09), 0x26); // year of century, BCD

    // Across a century boundary: 2100-01-01T00:00:00Z.
    let mut y2100 = rtc_at(4_102_444_800);
    write_reg(&mut y2100, 0x0B, B_24H);
    assert_eq!(read_reg(&mut y2100, 0x32), 21);
    assert_eq!(read_reg(&mut y2100, 0x09), 0x00);

    // A guest write is overwritten by the live value on read.
    write_reg(&mut rtc, 0x32, 0x42);
    assert_eq!(read_reg(&mut rtc, 0x32), 20);
}

#[test]
fn wide_and_off_window_accesses_walk_ports_without_panic() {
    let mut rtc = rtc_at(NOW);
    write_reg(&mut rtc, 0x0B, B_24H); // BCD, 24-hour
    rtc.write(0, &[0x04]);
    let mut pair = [0u8; 2];
    rtc.read(0, &mut pair);
    assert_eq!(pair, [0x04, 0x12]); // index then hours (noon, 24h)
    // Offsets past the window read zero and writes are ignored.
    let mut junk = [0xFF; 4];
    rtc.read(2, &mut junk);
    assert_eq!(junk, [0; 4]);
    rtc.write(9, &[0x00]);
    rtc.write(u64::MAX, &[0; 8]);
}
