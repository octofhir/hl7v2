//! Offline native-package validation; output contains coordinates and status, never field values.
use octofhir_hl7v2::{
    schema::{
        MatchLimits,
        fields::ValidationLimits,
        package::{Fingerprint, LoadLimits, OwnedPackage},
    },
    wire::{Message, ParseOptions},
};
use std::{
    ffi::OsString,
    fs::File,
    io::{Read, Write},
    path::Path,
    process::ExitCode,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const USAGE: &str = "usage: hl7v2-validate PACKAGE EXPECTED_SHA256 ER7_FILE [ER7_FILE ...]";

fn main() -> ExitCode {
    match run(std::env::args_os().skip(1), &mut std::io::stdout().lock()) {
        Ok(status) => ExitCode::from(status),
        Err(error) => {
            let _ = writeln!(std::io::stderr().lock(), "hl7v2-validate: {error}");
            ExitCode::from(2)
        }
    }
}
fn run(mut args: impl Iterator<Item = OsString>, out: &mut impl Write) -> Result<u8> {
    let package_path = args.next().ok_or(USAGE)?;
    let expected = fingerprint(args.next().ok_or(USAGE)?)?;
    let first = args.next().ok_or(USAGE)?;
    // Activate once before opening any message; all files reuse these exact compiled plans.
    let package =
        OwnedPackage::read_from(File::open(package_path)?, expected, LoadLimits::default())?;
    write!(out, "schema_digest=")?;
    for byte in package.digest() {
        write!(out, "{byte:02x}")?;
    }
    writeln!(out)?;
    let mut status = 0;
    for (index, path) in std::iter::once(first).chain(args).enumerate() {
        let number = index + 1;
        writeln!(out, "input={number}")?;
        status = status.max(validate_file(&package, Path::new(&path), out)?);
    }
    Ok(status)
}
fn fingerprint(value: OsString) -> Result<Fingerprint> {
    let text = value
        .to_str()
        .ok_or("expected 64 ASCII hexadecimal digits")?;
    if text.len() != 64 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("expected 64 ASCII hexadecimal digits".into());
    }
    let mut digest = [0; 32];
    for (byte, pair) in digest.iter_mut().zip(text.as_bytes().as_chunks::<2>().0) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair)?, 16)?;
    }
    Ok(digest)
}
fn validate_file(package: &OwnedPackage, path: &Path, out: &mut impl Write) -> Result<u8> {
    let options = ParseOptions::default();
    let cap = u64::try_from(options.max_bytes)?
        .checked_add(1)
        .ok_or("input limit overflow")?;
    let read = || -> std::io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        File::open(path)?.take(cap).read_to_end(&mut bytes)?;
        Ok(bytes)
    };
    let bytes = match read() {
        Ok(bytes) => bytes,
        Err(error) => {
            writeln!(out, "io_error={:?}", error.kind())?;
            return Ok(2);
        }
    };
    let message = match Message::parse_with_options(bytes, options) {
        Ok(message) => message,
        Err(error) => {
            writeln!(out, "wire_error={error:?}")?;
            return Ok(1);
        }
    };
    writeln!(
        out,
        "bytes={} segments={} wire_diagnostics={} wire_omitted={}",
        message.original().len(),
        message.segments().len(),
        message.diagnostics().len(),
        message.omitted_diagnostics()
    )?;
    for diagnostic in message.diagnostics() {
        writeln!(out, "wire_diagnostic={diagnostic:?}")?;
    }
    let view = match package.catalog().group(&message, MatchLimits::default()) {
        Ok(view) => view,
        Err(error) => {
            writeln!(out, "structure_error={error:?}")?;
            return Ok(1);
        }
    };
    let report = package
        .fields()
        .validate(&view, ValidationLimits::default());
    for issue in &report.issues {
        writeln!(out, "field_issue={issue:?}")?;
    }
    writeln!(
        out,
        "schema_valid={} complete={} issues={} omitted={} steps={} bytes_visited={}",
        report.is_valid(),
        report.complete,
        report.issues.len(),
        report.omitted,
        report.steps,
        report.bytes
    )?;
    Ok(u8::from(!report.is_valid()))
}
