//! Images spread over several files: reading `.1.iso`, `.2.iso`, … as one, and writing an
//! image that is cut into parts when it outgrows FAT32.

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::error::{at, Result};

/// Where split images are cut: just under 4 GiB and on a sector boundary. The same point
/// xdvdfs and the Xbox loaders that read split images use.
pub const SPLIT_SIZE: u64 = 0xFFBF_6000;

/// The final name of part `index` (0-based) of a split image whose unsplit name is `path`:
/// `Game.iso` → `Game.1.iso`, `Game.2.iso`, …
pub fn part_name(path: &Path, index: usize) -> PathBuf {
    let ext = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_extension(format!("{}.{ext}", index + 1))
}

/// Several files read back to back as one stream.
pub struct MultiFile {
    /// Each part with its size and the reader's current position in it.
    parts: Vec<(BufReader<File>, u64, u64)>,
    len: u64,
    pos: u64,
}

impl MultiFile {
    pub fn open(paths: &[PathBuf]) -> Result<Self> {
        let mut parts = Vec::new();
        let mut len = 0;
        for p in paths {
            let f = File::open(p).map_err(at(p))?;
            let size = f.metadata().map_err(at(p))?.len();
            len += size;
            parts.push((BufReader::with_capacity(1024 * 1024, f), size, 0));
        }
        Ok(Self { parts, len, pos: 0 })
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Read for MultiFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut start = 0;
        for (f, size, cur) in &mut self.parts {
            if self.pos < start + *size {
                let within = self.pos - start;
                // Seeking drops BufReader's buffer, so only seek when not already there.
                if *cur != within {
                    f.seek(SeekFrom::Start(within))?;
                }
                let n = (buf.len() as u64).min(*size - within) as usize;
                let n = f.read(&mut buf[..n])?;
                *cur = within + n as u64;
                self.pos += n as u64;
                return Ok(n);
            }
            start += *size;
        }
        Ok(0)
    }
}

impl Seek for MultiFile {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        self.pos = resolve(to, self.pos, self.len)?;
        Ok(self.pos)
    }
}

/// The absolute position a seek on a stream of `len` bytes at `pos` lands on.
pub(crate) fn resolve(to: SeekFrom, pos: u64, len: u64) -> io::Result<u64> {
    let target = match to {
        SeekFrom::Start(o) => Some(o),
        SeekFrom::End(d) => len.checked_add_signed(d),
        SeekFrom::Current(d) => pos.checked_add_signed(d),
    };
    target.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before start of image"))
}

/// Writes an image into temporary files next to its destination, cutting it into parts of
/// [`SPLIT_SIZE`] when splitting is on. Nothing appears under the final names until
/// [`SplitWriter::finish`] renames the parts into place.
pub struct SplitWriter {
    output: PathBuf,
    part_size: u64,
    parts: Vec<(BufWriter<File>, PathBuf, u64)>,
    pos: u64,
    len: u64,
}

impl SplitWriter {
    pub fn new(output: &Path, split: bool) -> Self {
        Self {
            output: output.to_path_buf(),
            part_size: if split { SPLIT_SIZE } else { u64::MAX },
            parts: Vec::new(),
            pos: 0,
            len: 0,
        }
    }

    /// The temporary files written so far.
    pub fn temp_paths(&self) -> Vec<PathBuf> {
        self.parts.iter().map(|(_, p, _)| p.clone()).collect()
    }

    /// The names the parts get once finished: the output itself, or numbered parts.
    pub fn final_paths(&self) -> Vec<PathBuf> {
        match self.parts.len() {
            0 | 1 => vec![self.output.clone()],
            n => (0..n).map(|i| part_name(&self.output, i)).collect(),
        }
    }

    fn temp_name(&self, index: usize) -> PathBuf {
        let mut name = self.output.as_os_str().to_owned();
        name.push(format!(".part{index}"));
        PathBuf::from(name)
    }

    fn part(&mut self, index: usize) -> io::Result<&mut (BufWriter<File>, PathBuf, u64)> {
        while self.parts.len() <= index {
            let path = self.temp_name(self.parts.len());
            let f = File::create(&path)?;
            self.parts.push((BufWriter::with_capacity(4 * 1024 * 1024, f), path, 0));
        }
        Ok(&mut self.parts[index])
    }

    /// Flushes and closes the temporary files, returning their paths.
    pub fn close(&mut self) -> Result<Vec<PathBuf>> {
        for (w, p, _) in &mut self.parts {
            w.flush().map_err(at(p))?;
            w.get_ref().sync_all().map_err(at(p))?;
        }
        Ok(self.temp_paths())
    }

    /// Renames the temporary files to their final names, replacing existing files.
    pub fn finish(mut self) -> Result<Vec<PathBuf>> {
        self.close()?;
        let finals = self.final_paths();
        let temps = self.temp_paths();
        drop(self.parts);
        for (tmp, fin) in temps.iter().zip(&finals) {
            if fin.exists() {
                std::fs::remove_file(fin).map_err(at(fin))?;
            }
            std::fs::rename(tmp, fin).map_err(at(fin))?;
        }
        Ok(finals)
    }

    /// Deletes the temporary files.
    pub fn discard(self) {
        let temps = self.temp_paths();
        drop(self.parts);
        for t in temps {
            let _ = std::fs::remove_file(t);
        }
    }
}

impl Write for SplitWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let index = (self.pos / self.part_size) as usize;
        let within = self.pos % self.part_size;
        let n = (buf.len() as u64).min(self.part_size - within) as usize;
        let part = self.part(index)?;
        if part.2 != within {
            part.0.seek(SeekFrom::Start(within))?;
        }
        let n = part.0.write(&buf[..n])?;
        part.2 = within + n as u64;
        self.pos += n as u64;
        self.len = self.len.max(self.pos);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        for (w, _, _) in &mut self.parts {
            w.flush()?;
        }
        Ok(())
    }
}

impl Seek for SplitWriter {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        self.pos = resolve(to, self.pos, self.len)?;
        Ok(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_parts_and_reads_them_back() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("Game.iso");
        let mut w = SplitWriter::new(&out, true);
        w.part_size = 10;
        w.write_all(b"0123456789abcdefghijXYZ").unwrap();
        w.seek(SeekFrom::Start(8)).unwrap();
        w.write_all(b"##").unwrap();
        let finals = w.finish().unwrap();
        assert_eq!(
            finals,
            vec![dir.path().join("Game.1.iso"), dir.path().join("Game.2.iso"), dir.path().join("Game.3.iso")]
        );
        assert_eq!(std::fs::read(&finals[0]).unwrap(), b"01234567##");

        let mut r = MultiFile::open(&finals).unwrap();
        assert_eq!(r.len(), 23);
        let mut buf = [0u8; 6];
        r.seek(SeekFrom::Start(8)).unwrap();
        r.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"##abcd");
    }

    #[test]
    fn unsplit_output_keeps_its_name() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("Game.iso");
        let mut w = SplitWriter::new(&out, false);
        w.write_all(b"data").unwrap();
        assert_eq!(w.finish().unwrap(), vec![out.clone()]);
        assert_eq!(std::fs::read(out).unwrap(), b"data");
    }
}
