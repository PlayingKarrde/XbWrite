//! A wrapper around an xdvdfs source file system that leaves out the dashboard update folder,
//! reports copy progress and stops when cancelled.

use std::sync::atomic::{AtomicBool, Ordering};

use xdvdfs::blockdev::BlockDeviceWrite;
use xdvdfs::write::fs::{FileEntry, FilesystemCopier, FilesystemHierarchy, PathRef};

/// Name of the folder holding the dashboard update every disc carries. Consoles that already
/// have a newer dashboard, softmodded consoles and emulators never use it.
pub const SYSTEM_UPDATE: &str = "$SystemUpdate";

pub fn is_system_update(name: &str) -> bool {
    name.eq_ignore_ascii_case(SYSTEM_UPDATE)
}

/// Large files are copied in pieces of this size so progress and cancelling stay responsive.
const CHUNK: u64 = 8 * 1024 * 1024;

pub struct TrimFs<'a, F> {
    pub inner: F,
    pub skip_system_update: bool,
    pub cancel: &'a AtomicBool,
    /// Called with the number of bytes after each piece of file data is copied.
    pub on_copy: &'a mut (dyn FnMut(u64) + Send + Sync),
}

#[derive(Debug, thiserror::Error)]
pub enum TrimError<E> {
    #[error(transparent)]
    Inner(E),
    #[error("cancelled")]
    Cancelled,
}

impl<F: FilesystemHierarchy> FilesystemHierarchy for TrimFs<'_, F> {
    type Error = F::Error;

    fn read_dir(&mut self, path: PathRef<'_>) -> Result<Vec<FileEntry>, Self::Error> {
        let mut entries = self.inner.read_dir(path)?;
        if self.skip_system_update && path.is_root() {
            entries.retain(|e| !is_system_update(&e.name));
        }
        Ok(entries)
    }

    fn clear_cache(&mut self) -> Result<(), Self::Error> {
        self.inner.clear_cache()
    }
}

impl<W, F> FilesystemCopier<W> for TrimFs<'_, F>
where
    W: BlockDeviceWrite + ?Sized,
    F: FilesystemCopier<W>,
{
    type Error = TrimError<F::Error>;

    fn copy_file_in(
        &mut self,
        src: PathRef<'_>,
        dest: &mut W,
        input_offset: u64,
        output_offset: u64,
        size: u64,
    ) -> Result<u64, Self::Error> {
        let mut done = 0;
        while done < size {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(TrimError::Cancelled);
            }
            let n = (size - done).min(CHUNK);
            let copied = self
                .inner
                .copy_file_in(src, dest, input_offset + done, output_offset + done, n)
                .map_err(TrimError::Inner)?;
            (self.on_copy)(copied);
            done += copied;
            if copied < n {
                break;
            }
        }
        Ok(done)
    }
}
