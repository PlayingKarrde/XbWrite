use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::{Error, Result};

/// Size of the pieces file data is read in, so progress and cancelling stay responsive.
pub(crate) const CHUNK: usize = 4 * 1024 * 1024;

/// Walks `size` bytes in pieces of at most `chunk`, calling `f(pos, len)` for each. Stops with
/// [`Error::Cancelled`] once `cancel` is set.
pub(crate) fn for_each_chunk(
    size: u64,
    chunk: usize,
    cancel: &AtomicBool,
    mut f: impl FnMut(u64, usize) -> Result<()>,
) -> Result<()> {
    let mut pos = 0;
    while pos < size {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let n = (size - pos).min(chunk as u64) as usize;
        f(pos, n)?;
        pos += n as u64;
    }
    Ok(())
}

pub(crate) fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    b.get(o..o + 2).map(|s| u16::from_le_bytes(s.try_into().unwrap()))
}

pub(crate) fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()))
}

pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.2} {}", UNITS[u])
    }
}

/// `part / whole` as a whole percentage, for "saves 63%" style labels.
pub fn percent(part: u64, whole: u64) -> u64 {
    if whole == 0 {
        0
    } else {
        ((part as f64 / whole as f64) * 100.0).round() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_percentages() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(3 * 1024 * 1024), "3.00 MB");
        assert_eq!(percent(1, 4), 25);
        assert_eq!(percent(1, 0), 0);
    }
}
