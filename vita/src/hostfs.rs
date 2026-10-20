//! File access that the USB host file system (`host0:`) serves reliably: open, sequential read, create, write
//! and rename. Stat-style requests fail on the host side and stall the device's single USB channel, so none are
//! used.

use std::fs::File;
use std::io::{Read, Write};

/// Whole file, at most `cap` bytes.
pub fn read(path: &str, cap: usize) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path).ok()?.take(cap as u64 + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() <= cap).then_some(bytes)
}

/// Writes through a temporary file and a rename, so a reader on the computer never sees a partial file. The
/// directory must exist.
pub fn write(path: &str, bytes: &[u8]) -> Result<(), String> {
    let temp = format!("{path}.tmp");
    let mut f = File::create(&temp).map_err(|e| format!("{temp}: {e}"))?;
    f.write_all(bytes).map_err(|e| format!("{temp}: {e}"))?;
    drop(f);
    std::fs::rename(&temp, path).map_err(|e| format!("{path}: {e}"))
}
