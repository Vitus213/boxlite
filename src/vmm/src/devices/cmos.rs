// Copyright 2026 BoxLite Contributors
// SPDX-License-Identifier: Apache-2.0

//! The CMOS real-time clock (RTC) of a PC, at ports 0x70 and 0x71.
//!
//! The guest writes a register number to port 0x70, then reads or writes that
//! register through port 0x71. Linux reads the date and time from it once at
//! boot to set its system clock.
//!
//! The time is an injected [`Clock`] plus an offset, so a time the guest
//! writes stays set and keeps running. While the guest sets the time, the
//! clock stops and the registers hold what it wrote; when the clock restarts,
//! it starts half a second into the written second, because Linux writes the
//! time half a second early (`set_offset_nsec` in `rtc-cmos.c`).
//!
//! Not modelled: 12-hour mode, and IRQ 8, so alarm, update and periodic
//! interrupts never fire.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::bus::{BusDevice, UNCLAIMED_READ};

/// The clock the RTC follows, in nanoseconds since the Unix epoch.
pub(crate) type Clock = Box<dyn Fn() -> i64 + Send>;

/// The host wall clock, in nanoseconds since the Unix epoch.
pub(crate) fn host_clock() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_nanos()).unwrap_or(i64::MAX),
        Err(before) => i64::try_from(before.duration().as_nanos()).map_or(i64::MIN, |ns| -ns),
    }
}

// Register numbers and bits carry their names from Linux's
// `include/linux/mc146818rtc.h`.
const RTC_SECONDS: usize = 0x00;
const RTC_MINUTES: usize = 0x02;
const RTC_HOURS: usize = 0x04;
const RTC_DAY_OF_WEEK: usize = 0x06;
const RTC_DAY_OF_MONTH: usize = 0x07;
const RTC_MONTH: usize = 0x08;
const RTC_YEAR: usize = 0x09;
const RTC_REG_A: usize = 0x0a;
const RTC_REG_B: usize = 0x0b;
const RTC_REG_C: usize = 0x0c;
const RTC_REG_D: usize = 0x0d;
/// The century byte PC firmware uses; Linux reads it only through ACPI.
const CENTURY: usize = 0x32;
const TIME_REGISTERS: [usize; 8] = [
    RTC_SECONDS,
    RTC_MINUTES,
    RTC_HOURS,
    RTC_DAY_OF_WEEK,
    RTC_DAY_OF_MONTH,
    RTC_MONTH,
    RTC_YEAR,
    CENTURY,
];

/// Register A: update in progress, which this model never reports.
const RTC_UIP: u8 = 0x80;
/// Register A: the divider bits. Above the 32.768 kHz time base, they hold
/// the divider in reset and the clock stops.
const RTC_DIV_CTL: u8 = 0x70;
const RTC_REF_CLCK_32KHZ: u8 = 0x20;
/// Register A at power-on: the 32.768 kHz time base and a 1024 Hz periodic rate.
const REG_A_DEFAULT: u8 = 0x26;
/// Register B: SET stops the clock so the guest can write the time.
const RTC_SET: u8 = 0x80;
/// Register B: binary instead of BCD time registers.
const RTC_DM_BINARY: u8 = 0x04;
/// Register B: 24-hour mode, the only mode modelled.
const RTC_24H: u8 = 0x02;
/// Register D: the RAM and the time are valid.
const RTC_VRT: u8 = 0x80;

const NANOS_PER_SECOND: i64 = 1_000_000_000;
const SECONDS_PER_DAY: i64 = 86_400;

/// The CMOS RTC and its 128 bytes of CMOS RAM.
pub(crate) struct CmosRtc {
    /// The register the next data-port access selects.
    index: usize,
    /// The status registers, NVRAM, and the time while the clock is stopped.
    registers: [u8; 128],
    /// Guest time minus the clock, in nanoseconds.
    offset_from_clock: i64,
    /// Whether the guest wrote the century byte, which then sets the century.
    century_written: bool,
    clock: Clock,
}

impl CmosRtc {
    pub(crate) fn new(clock: Clock) -> Self {
        let mut registers = [0; 128];
        registers[RTC_REG_A] = REG_A_DEFAULT;
        registers[RTC_REG_B] = RTC_24H;
        registers[RTC_REG_D] = RTC_VRT;
        Self {
            index: 0,
            registers,
            offset_from_clock: 0,
            century_written: false,
            clock,
        }
    }

    /// Whether the time advances: SET is clear and the divider is not in reset.
    fn clock_running(&self) -> bool {
        self.registers[RTC_REG_B] & RTC_SET == 0
            && self.registers[RTC_REG_A] & RTC_DIV_CTL <= RTC_REF_CLCK_32KHZ
    }

    fn guest_time_nanos(&self) -> i64 {
        (self.clock)().saturating_add(self.offset_from_clock)
    }

    fn read_register(&self, index: usize) -> u8 {
        match index {
            _ if TIME_REGISTERS.contains(&index) && self.clock_running() => {
                self.time_register(index, self.guest_time_nanos())
            }
            RTC_REG_A => self.registers[RTC_REG_A] & !RTC_UIP,
            RTC_REG_C => 0,
            _ => self.registers[index],
        }
    }

    fn write_register(&mut self, index: usize, value: u8) {
        let was_running = self.clock_running();
        match index {
            RTC_REG_A => self.registers[RTC_REG_A] = value & !RTC_UIP,
            RTC_REG_B => self.registers[RTC_REG_B] = value | RTC_24H,
            RTC_REG_C | RTC_REG_D => {}
            _ if TIME_REGISTERS.contains(&index) => {
                self.century_written |= index == CENTURY;
                if was_running {
                    // Change one field of the running time, and keep how far
                    // into its second the clock was.
                    let now = self.guest_time_nanos();
                    self.freeze_time(now);
                    self.registers[index] = value;
                    self.restart_from_registers(now.rem_euclid(NANOS_PER_SECOND));
                } else {
                    self.registers[index] = value;
                }
            }
            _ => self.registers[index] = value,
        }
        match (was_running, self.clock_running()) {
            (true, false) => self.freeze_time(self.guest_time_nanos()),
            // Linux writes the time half a second before that time is due,
            // whether it restarts the clock through SET or the divider.
            (false, true) => self.restart_from_registers(NANOS_PER_SECOND / 2),
            _ => {}
        }
    }

    /// Writes the time at `nanos` into the time registers, which keep it
    /// until the clock restarts.
    fn freeze_time(&mut self, nanos: i64) {
        for index in TIME_REGISTERS {
            self.registers[index] = self.time_register(index, nanos);
        }
    }

    /// Runs the clock on from the time in the registers, `fraction_nanos`
    /// into its second. A time the registers do not express leaves the clock
    /// where it was.
    fn restart_from_registers(&mut self, fraction_nanos: i64) {
        if let Some(nanos) = self.registers_time_nanos(fraction_nanos) {
            self.offset_from_clock = nanos.saturating_sub((self.clock)());
        }
    }

    /// The time in the registers, `fraction_nanos` into its second, or `None`
    /// when a field is out of range.
    fn registers_time_nanos(&self, fraction_nanos: i64) -> Option<i64> {
        let field = |index: usize| self.decode(self.registers[index]);
        let (second, minute, hour) = (field(RTC_SECONDS)?, field(RTC_MINUTES)?, field(RTC_HOURS)?);
        let (day, month, year) = (
            field(RTC_DAY_OF_MONTH)?,
            field(RTC_MONTH)?,
            field(RTC_YEAR)?,
        );
        if second > 59
            || minute > 59
            || hour > 23
            || !(1..=31).contains(&day)
            || !(1..=12).contains(&month)
            || year > 99
        {
            return None;
        }
        let century = if self.century_written {
            field(CENTURY).filter(|century| *century <= 99)?
        } else if year < 70 {
            20
        } else {
            19
        };
        let days = days_from_civil(century * 100 + year, month, day);
        let seconds = days * SECONDS_PER_DAY + hour * 3600 + minute * 60 + second;
        seconds
            .checked_mul(NANOS_PER_SECOND)?
            .checked_add(fraction_nanos)
    }

    /// Time register `index` at guest time `nanos`, encoded as register B says.
    fn time_register(&self, index: usize, nanos: i64) -> u8 {
        let seconds = nanos.div_euclid(NANOS_PER_SECOND);
        let days = seconds.div_euclid(SECONDS_PER_DAY);
        let of_day = seconds.rem_euclid(SECONDS_PER_DAY);
        let (year, month, day) = civil_from_days(days);
        let value = match index {
            RTC_SECONDS => of_day % 60,
            RTC_MINUTES => of_day / 60 % 60,
            RTC_HOURS => of_day / 3600,
            // 1970-01-01 was a Thursday, and Sunday is 1.
            RTC_DAY_OF_WEEK => (days + 4).rem_euclid(7) + 1,
            RTC_DAY_OF_MONTH => day,
            RTC_MONTH => month,
            RTC_YEAR => year.rem_euclid(100),
            _ => year.div_euclid(100).rem_euclid(100),
        };
        self.encode(value as u8)
    }

    fn binary_mode(&self) -> bool {
        self.registers[RTC_REG_B] & RTC_DM_BINARY != 0
    }

    fn encode(&self, value: u8) -> u8 {
        if self.binary_mode() {
            value
        } else {
            ((value / 10) << 4) | (value % 10)
        }
    }

    /// A time register value, or `None` for a byte that is not BCD in BCD mode.
    fn decode(&self, raw: u8) -> Option<i64> {
        if self.binary_mode() {
            return Some(raw.into());
        }
        let (tens, ones) = (raw >> 4, raw & 0x0f);
        (tens <= 9 && ones <= 9).then(|| i64::from(tens * 10 + ones))
    }
}

impl BusDevice for CmosRtc {
    fn read(&mut self, offset: u64, data: &mut [u8]) {
        match (offset, data) {
            (1, [value]) => *value = self.read_register(self.index),
            // The index port is write-only, and registers take byte accesses.
            (_, data) => data.fill(UNCLAIMED_READ),
        }
    }

    fn write(&mut self, offset: u64, data: &[u8]) {
        match (offset, data) {
            // Bit 7 masks NMI on a PC; there is no NMI source to mask here.
            (0, [value]) => self.index = usize::from(value & 0x7f),
            (1, [value]) => self.write_register(self.index, *value),
            _ => {}
        }
    }
}

/// The proleptic Gregorian date `days` after 1970-01-01, as Howard Hinnant's
/// `civil_from_days` computes it.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    };
    (era * 400 + year_of_era + i64::from(month <= 2), month, day)
}

/// Days from 1970-01-01 to a proleptic Gregorian date, the inverse of
/// [`civil_from_days`]; days past the end of a month run into the next.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
#[path = "../../tests/cmos/mod.rs"]
mod tests;
