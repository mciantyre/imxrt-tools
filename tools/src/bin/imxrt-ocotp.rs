// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: Copyright 2026 Ian McIntyre

use std::io::{self, BufRead, Write};

use clap::{error::ErrorKind, CommandFactory, Parser, Subcommand, ValueEnum};
use ocotp::{FuseAddress, Ocotp};

/// Read and write fuses on i.MXRT MCUs using the OCOTP controller
#[derive(Parser)]
#[command(version, about, long_about)]
struct Cli {
    /// The i.MXRT MCU variant
    #[arg(ignore_case = true)]
    mcu: Mcu,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Read a fuse value
    Read {
        /// The fuse address (hex with 0x prefix, or decimal)
        #[arg(long = "fuse-address", value_parser = parse_u16)]
        fuse_address: u16,
    },
    /// Write a fuse value (IRREVERSIBLE - bits set cannot be cleared)
    ///
    /// If you're writing a redundant fuse, this will not lock the fuse. You
    /// may be able to set other bits sometime later. However, if this is an
    /// ECC fuse, the write will automatically lock the fuse.
    Write {
        /// The fuse address (hex 0x, binary 0b, or decimal)
        #[arg(long = "fuse-address", value_parser = parse_u16)]
        fuse_address: Option<u16>,

        /// The value to write (hex 0x, binary 0b, or decimal)
        #[arg(long = "fuse-value", value_parser = parse_u32)]
        fuse_value: Option<u32>,

        /// Perform validation and confirmation without writing
        ///
        /// This will attempt to connect with the debug probe. However,
        /// it does not engage with the OCOTP.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Clone, ValueEnum)]
#[value(rename_all = "UPPER")]
enum Mcu {
    Imxrt1160,
    Imxrt1170,
}

impl Mcu {
    fn ocotp(&self) -> &'static Ocotp {
        match self {
            Self::Imxrt1160 => &ocotp::IMXRT1160,
            Self::Imxrt1170 => &ocotp::IMXRT1170,
        }
    }

    fn probe_rs_name(&self) -> &'static str {
        match self {
            Self::Imxrt1160 => "MIMXRT1160",
            Self::Imxrt1170 => "MIMXRT1170",
        }
    }
}

fn parse_u16(s: &str) -> Result<u16, String> {
    parse_u32(s).and_then(|v| u16::try_from(v).map_err(|err| format!("{err}")))
}

fn parse_u32(s: &str) -> Result<u32, String> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16).map_err(|e| format!("Invalid hex value: {e}"))
    } else if let Some(bin) = s.strip_prefix("0b").or_else(|| s.strip_prefix("0B")) {
        u32::from_str_radix(bin, 2).map_err(|e| format!("Invalid binary value: {e}"))
    } else {
        s.parse::<u32>()
            .map_err(|e| format!("Invalid decimal value: {e}"))
    }
}

fn prompt_double_entry<T, F>(prompt_name: &str, parse_fn: F) -> Result<T, String>
where
    T: PartialEq + std::fmt::Debug,
    F: Fn(&str) -> Result<T, String>,
{
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    write!(stdout, "Enter {prompt_name}      : ").unwrap();
    stdout.flush().unwrap();
    let mut first = String::new();
    stdin.lock().read_line(&mut first).unwrap();
    let first_val = parse_fn(first.trim())?;

    write!(stdout, "Enter {prompt_name} again: ").unwrap();
    stdout.flush().unwrap();
    let mut second = String::new();
    stdin.lock().read_line(&mut second).unwrap();
    let second_val = parse_fn(second.trim())?;

    if first_val != second_val {
        return Err(format!("{prompt_name} entries do not match"));
    }
    Ok(first_val)
}

fn prompt_write_confirmation(fuse_address: u16, fuse_value: u32) -> bool {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    writeln!(stdout, "\n=== FUSE WRITE CONFIRMATION ===").unwrap();
    writeln!(stdout).unwrap();
    writeln!(stdout, "Fuse Address:").unwrap();
    writeln!(stdout, "  Hex    : {fuse_address:#06X}").unwrap();
    writeln!(stdout, "  Decimal: {fuse_address}").unwrap();
    writeln!(stdout).unwrap();
    writeln!(stdout, "Fuse Value:").unwrap();
    writeln!(stdout, "  Hex    : {fuse_value:#010X}").unwrap();
    writeln!(stdout, "  Decimal: {fuse_value}").unwrap();
    writeln!(stdout).unwrap();
    writeln!(
        stdout,
        "WARNING: This operation is IRREVERSIBLE. Bits set cannot be cleared."
    )
    .unwrap();
    writeln!(
        stdout,
        "Additionally, if this is an ECC fuse, this write will auto-lock the fuse."
    )
    .unwrap();
    writeln!(stdout).unwrap();
    write!(stdout, "Proceed with write? [y/N]: ").unwrap();
    stdout.flush().unwrap();

    let mut input = String::new();
    stdin.lock().read_line(&mut input).unwrap();
    input.trim().eq_ignore_ascii_case("y")
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Command::Read { fuse_address } => {
            let fuse = match FuseAddress::new(fuse_address) {
                Some(f) => f,
                None => Cli::command()
                    .error(
                        ErrorKind::InvalidValue,
                        format!("Invalid fuse address: {fuse_address:#06X}"),
                    )
                    .exit(),
            };

            let mut session =
                match probe_rs::Session::auto_attach(cli.mcu.probe_rs_name(), Default::default()) {
                    Ok(session) => session,
                    Err(err) => Cli::command()
                        .error(
                            ErrorKind::Io,
                            format!("{err} {err:?}\nIs your MCU connected to your debugger?"),
                        )
                        .exit(),
                };

            let mut core = session.core(0).unwrap();

            match ocotp::read_fuse(cli.mcu.ocotp(), fuse, &mut core) {
                Ok(value) => {
                    println!("Fuse {fuse_address:#06X}: {value:#010X} ({value})");
                }
                Err(err) => Cli::command()
                    .error(ErrorKind::Io, format!("Failed to read fuse: {err}"))
                    .exit(),
            }
        }
        Command::Write {
            fuse_address,
            fuse_value,
            dry_run,
        } => {
            let fuse_address = match fuse_address {
                Some(addr) => addr,
                None => match prompt_double_entry("fuse address", parse_u16) {
                    Ok(addr) => addr,
                    Err(err) => Cli::command().error(ErrorKind::InvalidValue, err).exit(),
                },
            };

            let fuse = match FuseAddress::new(fuse_address) {
                Some(f) => f,
                None => Cli::command()
                    .error(
                        ErrorKind::InvalidValue,
                        format!("Invalid fuse address: {fuse_address:#06X}"),
                    )
                    .exit(),
            };

            let fuse_value = match fuse_value {
                Some(val) => val,
                None => match prompt_double_entry("fuse value", parse_u32) {
                    Ok(val) => val,
                    Err(err) => Cli::command().error(ErrorKind::InvalidValue, err).exit(),
                },
            };

            if !prompt_write_confirmation(fuse_address, fuse_value) {
                println!("Write aborted.");
                return;
            }

            let mut session =
                match probe_rs::Session::auto_attach(cli.mcu.probe_rs_name(), Default::default()) {
                    Ok(session) => session,
                    Err(err) => Cli::command()
                        .error(
                            ErrorKind::Io,
                            format!("{err} {err:?}\nIs your MCU connected to your debugger?"),
                        )
                        .exit(),
                };

            let mut core = session.core(0).unwrap();

            if dry_run {
                println!("Dry run: would write {fuse_value:#010X} to fuse {fuse_address:#06X}");
                return;
            }

            match ocotp::write_fuse(cli.mcu.ocotp(), fuse, fuse_value, &mut core) {
                Ok(()) => {
                    println!("Successfully wrote {fuse_value:#010X} to fuse {fuse_address:#06X}");
                }
                Err(err) => Cli::command()
                    .error(ErrorKind::Io, format!("Failed to write fuse: {err}"))
                    .exit(),
            }
        }
    }
}
