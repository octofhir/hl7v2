#![cfg(feature = "schema-package")]
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;
const PIN: &str = "0161081c3eb15536d263ba1b1f357725b0efc43b7e2d02a58b541dda1e669cd1";
const MESSAGE: &[u8] = include_bytes!("fixtures/native-package.er7");

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
fn command(pin: &str, files: &[&Path]) -> Result<Output> {
    Ok(Command::new(env!("CARGO_BIN_EXE_hl7v2-validate"))
        .arg(fixture("native-package-v1.bin"))
        .arg(pin)
        .args(files)
        .output()?)
}
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Result<Self> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..128 {
            let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "hl7v2-offline-{}-{nonce}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err("cannot allocate a unique test directory".into())
    }
    fn write(&self, name: &str, bytes: &[u8]) -> Result<PathBuf> {
        let path = self.0.join(name);
        fs::write(&path, bytes)?;
        Ok(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn multiple_inputs_reuse_one_pin_report_source_coordinates_and_keep_values_private() -> Result {
    let scratch = Scratch::new()?;
    let invalid = scratch.write("invalid.er7", b"MSH|^~\\&|||||||ZZZ^Z01^SYNTHETIC|||TEST\rZZZ|NM|secret-nonnumeric\rNTE|1||private-note\r")?;
    let valid = fixture("native-package.er7");
    let output = command(&PIN.to_uppercase(), &[&valid, &invalid, &valid])?;
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let text = std::str::from_utf8(&output.stdout)?;
    assert_eq!(text.matches(&format!("schema_digest={PIN}")).count(), 1);
    assert_eq!(text.matches("schema_valid=true").count(), 2);
    assert_eq!(text.matches("schema_valid=false").count(), 1);
    assert!(
        text.contains(
            "kind: Lexical(Number), span: 47..64, path: [2, 2, 1, 0, 0], presence: Value"
        )
    );
    assert!(text.contains("input=3"));
    for value in [
        "secret-nonnumeric",
        "private-note",
        "0001.2300",
        "synthetic note",
    ] {
        assert!(!text.contains(value));
    }
    Ok(())
}

#[test]
fn activation_and_usage_fail_before_message_io() -> Result {
    let missing = Path::new("/nonexistent-octofhir-offline-message");
    for (pin, error) in [
        ("00".repeat(32), "DigestMismatch"),
        (
            "secret-invalid-pin".into(),
            "expected 64 ASCII hexadecimal digits",
        ),
    ] {
        let output = command(&pin, &[missing])?;
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let text = std::str::from_utf8(&output.stderr)?;
        assert!(text.contains(error));
        assert!(!text.contains("secret-invalid-pin"));
    }
    let output = Command::new(env!("CARGO_BIN_EXE_hl7v2-validate")).output()?;
    assert_eq!(output.status.code(), Some(2));
    assert!(std::str::from_utf8(&output.stderr)?.contains("usage: hl7v2-validate"));
    Ok(())
}

#[test]
fn wire_structure_report_caps_and_io_outcomes_remain_distinct() -> Result {
    let scratch = Scratch::new()?;
    let wire = scratch.write("wire.er7", b"invalid-message")?;
    let mut extra = MESSAGE.to_vec();
    extra.extend_from_slice(b"Z99|private-extension\r");
    let shape = scratch.write("shape.er7", &extra)?;
    let unterminated = scratch.write("unterminated.er7", &MESSAGE[..MESSAGE.len() - 1])?;
    let missing = scratch.0.join("missing.er7");
    let output = command(PIN, &[&wire, &shape, &unterminated, &missing])?;
    assert_eq!(output.status.code(), Some(2));
    let text = std::str::from_utf8(&output.stdout)?;
    assert!(text.contains("wire_error=Error { kind: Header"));
    assert!(text.contains("structure_error=Error { kind: UnexpectedSegment"));
    assert!(text.contains("wire_diagnostic=Diagnostic { kind: MissingTerminator"));
    assert!(text.contains("schema_valid=true"));
    assert!(text.contains("io_error=NotFound"));
    assert!(!text.contains("private-extension"));
    let mut repeated = b"MSH|^~\\&|||||||ZZZ^Z01^SYNTHETIC|||TEST\r".to_vec();
    for _ in 0..1030 {
        repeated.extend_from_slice(b"ZZZ|NM|private-invalid\r");
    }
    let capped = scratch.write("capped.er7", &repeated)?;
    let output = command(PIN, &[&capped])?;
    assert_eq!(output.status.code(), Some(1));
    let text = std::str::from_utf8(&output.stdout)?;
    assert!(text.contains("schema_valid=false complete=true issues=1024 omitted=6"));
    assert!(!text.contains("private-invalid"));
    let large = scratch.0.join("oversized.er7");
    fs::File::create(&large)?.set_len(8 * 1024 * 1024 + 1)?;
    let output = command(PIN, &[&large])?;
    assert_eq!(output.status.code(), Some(1));
    assert!(std::str::from_utf8(&output.stdout)?.contains("wire_error=Error { kind: Limit"));
    Ok(())
}
