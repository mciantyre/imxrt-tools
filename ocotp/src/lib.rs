// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: Copyright 2026 Ian McIntyre

//! A debug probe interface to the on-chip OTP controller.

use std::time::Duration;

use probe_rs::MemoryInterface;

/// Handle for a MCU's OCOPT.
pub struct Ocotp {
    /// Base address.
    addr: u64,
}

/// Handle for an 1160's OCOTP.
pub static IMXRT1160: Ocotp = Ocotp { addr: 0x40CA_C000 };
/// Handle for an 1170's OCOPT.
pub static IMXRT1170: Ocotp = Ocotp { addr: 0x40CA_C000 };

impl Ocotp {
    /// Absolute address to the `CTRL` register.
    const fn ctrl(&self) -> u64 {
        self.addr
    }
    /// Absolute address to the `DATA` register.
    const fn data(&self) -> u64 {
        self.addr + 0x20
    }
    /// Absolute address to the `READ_CTRL` register.
    const fn read_ctrl(&self) -> u64 {
        self.addr + 0x30
    }
    /// Absolute address to the `OUT_STATUS` register.
    const fn out_status(&self) -> u64 {
        self.addr + 0x90
    }
    /// Absolute address for the `READ_FUSE_DATA` register.
    const fn read_fuse_data(&self, offset: u64) -> u64 {
        assert!(offset < 4);
        self.addr + 0x100 + (offset * 0x10)
    }
}

/// A fuse address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct FuseAddress(u16);

impl FuseAddress {
    /// Construct a fuse address.
    ///
    /// Returns `None` if this doesn't represent a valid
    /// fuse address.
    pub const fn new(addr: u16) -> Option<Self> {
        // Control register construction can perform
        // the validation.
        if Ctrl::from_fuse_address(addr).is_none() {
            None
        } else {
            Some(Self(addr))
        }
    }

    /// Returns the raw fuse address.
    pub const fn get(self) -> u16 {
        self.0
    }
}

/// The control (and status) register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
struct Ctrl(u32);

impl Ctrl {
    /// Form a control register value from a fuse address.
    ///
    /// The fuse address `addr` is the fuse you intend to read
    /// or write. Consult the fuse map for possible values.
    ///
    /// If the conversion is out of range, this returns `None`.
    const fn from_fuse_address(addr: u16) -> Option<Self> {
        let Some(addr) = addr.checked_sub(0x800) else {
            return None;
        };
        let Some(addr) = addr.checked_shr(4) else {
            return None;
        };
        Some(Ctrl(addr as u32))
    }

    /// Form a new control register with the write unlock sequence.
    const fn write_unlock(self) -> Self {
        Self(self.0 | CTRL_WRITE_UNLOCK)
    }
}

const CTRL_WRITE_UNLOCK: u32 = 0x3E77 << 16;
const CTRL_BUSY: u32 = 1 << 10;
const CTRL_ERROR: u32 = 1 << 11;

const READ_CTRL_READ_FUSE: u32 = 1 << 0;

const OUT_STATUS_DED: u32 = 1 << 10;
const OUT_STATUS_LOCKED: u32 = 1 << 11;
const OUT_STATUS_PROGFAIL: u32 = 1 << 12;

fn check_busy_or_error(ocotp: &Ocotp, probe: &mut dyn MemoryInterface) -> Result<(), Error> {
    let ctrl = probe
        .read_word_32(ocotp.ctrl())
        .context("reading CTRL to check BUSY/ERROR")?;

    if ctrl & CTRL_BUSY != 0 {
        let msg = format!("CTRL[BUSY] set before access. CTRL = {ctrl:#010X}");
        return Err(msg.into());
    }
    if ctrl & CTRL_ERROR != 0 {
        let msg = format!("CTRL[ERROR] set before access. CTRL = {ctrl:#010X}");
        return Err(msg.into());
    }

    Ok(())
}

fn wait_for_busy(ocotp: &Ocotp, probe: &mut dyn MemoryInterface) -> Result<(), Error> {
    while {
        let ctrl = probe.read_word_32(ocotp.ctrl()).context("Polling CTRL")?;
        if ctrl & CTRL_ERROR != 0 {
            let msg = format!("CTRL[ERROR] set during poll. CTRL = {ctrl:#010X}");
            return Err(msg.into());
        }
        ctrl & CTRL_BUSY != 0
    } {
        std::thread::sleep(Duration::from_millis(100));
    }

    Ok(())
}

/// Read the fuse.
///
/// This ignores shadow registers; it reads through
/// the controller.
pub fn read_fuse(
    ocotp: &Ocotp,
    fuse: FuseAddress,
    probe: &mut dyn MemoryInterface,
) -> Result<u32, Error> {
    check_busy_or_error(ocotp, probe)?;

    let ctrl = Ctrl::from_fuse_address(fuse.0).unwrap();
    probe
        .write_word_32(ocotp.ctrl(), ctrl.0)
        .context("Writing CTRL")?;
    probe.flush().context("Flush after CTRL write")?;

    probe
        .write_word_32(ocotp.read_ctrl(), READ_CTRL_READ_FUSE)
        .context("Writing READ_CTRL")?;
    probe.flush().context("Flush after READ_CTRL write")?;

    wait_for_busy(ocotp, probe)?;

    let result = probe
        .read_word_32(ocotp.read_fuse_data(0))
        .context("Reading READ_FUSE_DATA0")?;

    let out_status = probe
        .read_word_32(ocotp.out_status())
        .context("Reading OUT_STATUS")?;

    if out_status & OUT_STATUS_DED != 0 {
        let msg = format!("Read failed: DED. OUT_STATUS = {out_status:#010X}");
        return Err(msg.into());
    }

    Ok(result)
}

/// Write `value` to the given fuse.
///
/// Take care! Any bits set in `value` cannot be unwritten.
/// This call does not set the lock bit for redundant fuses.
pub fn write_fuse(
    ocotp: &Ocotp,
    fuse: FuseAddress,
    value: u32,
    probe: &mut dyn MemoryInterface,
) -> Result<(), Error> {
    check_busy_or_error(ocotp, probe)?;

    let ctrl = Ctrl::from_fuse_address(fuse.0).unwrap().write_unlock();
    probe
        .write_word_32(ocotp.ctrl(), ctrl.0)
        .context("Writing CTRL")?;
    probe.flush().context("Flush after CTRL write")?;

    probe
        .write_word_32(ocotp.data(), value)
        .context("Writing DATA")?;
    probe.flush().context("Flush after DATA write")?;

    wait_for_busy(ocotp, probe)?;

    let out_status = probe
        .read_word_32(ocotp.out_status())
        .context("Reading OUT_STATUS")?;

    if out_status & OUT_STATUS_PROGFAIL != 0 {
        let msg = format!("Write failed: PROGFAIL. OUT_STATUS = {out_status:#010X}");
        return Err(msg.into());
    }
    if out_status & OUT_STATUS_LOCKED != 0 {
        let msg = format!("Write failed: LOCKED. OUT_STATUS = {out_status:#010X}");
        return Err(msg.into());
    }

    Ok(())
}

/// Errors returned from this interface.
///
/// Follow the error sources for additional context.
pub type Error = Box<dyn std::error::Error + 'static>;

#[derive(Debug)]
struct ErrorContext {
    what: &'static str,
    extra: Error,
}

impl std::fmt::Display for ErrorContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "{}", self.what)
    }
}

impl std::error::Error for ErrorContext {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&*self.extra)
    }
}

trait Context<T> {
    fn context(self, msg: &'static str) -> Result<T, Error>;
}

impl<T, E: std::error::Error + 'static> Context<T> for Result<T, E> {
    fn context(self, msg: &'static str) -> Result<T, Error> {
        self.map_err(|err| {
            Box::new(ErrorContext {
                what: msg,
                extra: err.into(),
            }) as _
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Ctrl;

    #[test]
    fn ctrl_from_rm_example() {
        let ctrl = Ctrl::from_fuse_address(0x960).unwrap();
        assert_eq!(ctrl.0, 0x16);
    }

    #[test]
    fn ctrl_from_min_fuse() {
        // A low address, non-reserved fuse shown in one
        // reference manual.
        let ctrl = Ctrl::from_fuse_address(0x840).unwrap();
        assert_eq!(ctrl.0, 4);
    }

    #[test]
    fn ctrl_from_max_fuse() {
        // Similar to the min test, but using a high address.
        let ctrl = Ctrl::from_fuse_address(0x1800).unwrap();
        assert_eq!(ctrl.0, 0x100);
    }
}
