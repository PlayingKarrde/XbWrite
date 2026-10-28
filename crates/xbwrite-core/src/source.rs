//! Opening the things XbWrite reads: disc images (Redump ISO, XISO, CSO, split or not) and
//! extracted game folders.

use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use xdvdfs::blockdev::{BlockDeviceRead, OffsetWrapper, XDVDFSOffsets};
use xdvdfs::layout::VolumeDescriptor;

use crate::error::{at, format, Error, Result};
use crate::split::{part_name, resolve, MultiFile};

/// File extensions picked up when adding a folder.
pub const EXTENSIONS: [&str; 3] = ["iso", "xiso", "cso"];

/// Where an image's file system starts tells what kind of dump it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// A full Redump-style dump of an original disc, with the video partition in front.
    Redump(Xgd),
    /// The file system starts at the beginning: already an XISO.
    Xiso,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Xgd {
    Xgd1,
    Xgd2,
    Xgd3,
}

impl Xgd {
    pub fn label(self) -> &'static str {
        match self {
            Self::Xgd1 => "XGD1",
            Self::Xgd2 => "XGD2",
            Self::Xgd3 => "XGD3",
        }
    }
}

/// The raw bytes of an image file (or its parts), plain or CSO-compressed.
pub enum RawImage {
    Plain(MultiFile),
    Cso(Box<CsoStream>),
}

/// A CSO image read as the uncompressed stream it holds.
pub struct CsoStream {
    reader: ciso::read::CSOReader<io::Error, MultiFile>,
    pos: u64,
}

impl Read for RawImage {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(f) => Read::read(f, buf),
            Self::Cso(c) => {
                let n = (buf.len() as u64).min(c.reader.file_size().saturating_sub(c.pos)) as usize;
                c.reader.read_offset(c.pos, &mut buf[..n]).map_err(|e| match e {
                    ciso::layout::Error::Other(e) => e,
                    e => io::Error::other(format!("{e:?}")),
                })?;
                c.pos += n as u64;
                Ok(n)
            }
        }
    }
}

impl Seek for RawImage {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        match self {
            Self::Plain(f) => Seek::seek(f, to),
            Self::Cso(c) => {
                c.pos = resolve(to, c.pos, c.reader.file_size())?;
                Ok(c.pos)
            }
        }
    }
}

/// An image with its file system located: reads are relative to the start of the XDVDFS
/// partition.
pub type ImageDevice = OffsetWrapper<RawImage>;

/// The files making up an image: `Game.1.iso` brings along `Game.2.iso`, `Game.3.iso`, …
pub fn image_parts(path: &Path) -> Vec<PathBuf> {
    let Some(stem) = split_stem(path) else { return vec![path.to_path_buf()] };
    let ext = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    let unsplit = path.with_file_name(format!("{stem}.{ext}"));
    let mut parts: Vec<PathBuf> = (0..).map(|i| part_name(&unsplit, i)).take_while(|p| p.is_file()).collect();
    if parts.is_empty() {
        parts.push(path.to_path_buf());
    }
    parts
}

/// `Game` for `Game.1.iso` (the first part of a split image), otherwise `None`.
pub fn split_stem(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_string_lossy().into_owned();
    stem.strip_suffix(".1").map(str::to_string)
}

/// A later part of a split image (`Game.2.iso`, …), which is read through its first part.
pub fn is_later_part(path: &Path) -> bool {
    let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else { return false };
    match stem.rsplit_once('.') {
        Some((_, n)) => n.parse::<u32>().is_ok_and(|n| n > 1),
        None => false,
    }
}

/// Whether `path` names a CSO image.
pub fn is_cso(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("cso"))
}

/// Opens an image and finds its file system.
pub fn open_image(path: &Path) -> Result<(ImageDevice, Layout)> {
    open_image_parts(&image_parts(path), is_cso(path))
}

/// Opens an image from explicit parts (used to read back freshly written temp files).
pub fn open_image_parts(parts: &[PathBuf], cso: bool) -> Result<(ImageDevice, Layout)> {
    let first = parts.first().cloned().unwrap_or_default();
    let multi = MultiFile::open(parts)?;
    let raw = if cso {
        let reader = ciso::read::CSOReader::new(multi)
            .map_err(|e| Error::Format(format!("{}: not a valid CSO image ({e:?})", first.display())))?;
        RawImage::Cso(Box::new(CsoStream { reader, pos: 0 }))
    } else {
        RawImage::Plain(multi)
    };
    let dev = OffsetWrapper::new(raw)
        .map_err(|_| Error::NotXbox("no Xbox file system (XDVDFS) found. Is this an Xbox game image?".into()))?;
    let layout = match dev.get_offset() {
        XDVDFSOffsets::XISO => Layout::Xiso,
        XDVDFSOffsets::XGD1 => Layout::Redump(Xgd::Xgd1),
        XDVDFSOffsets::XGD2 => Layout::Redump(Xgd::Xgd2),
        XDVDFSOffsets::XGD3 => Layout::Redump(Xgd::Xgd3),
    };
    Ok((dev, layout))
}

/// Whether a folder is an extracted game (it holds `default.xbe`).
pub fn is_game_folder(dir: &Path) -> bool {
    find_ci(dir, "default.xbe").is_some()
}

/// The entry of `dir` named `name`, ignoring case.
pub fn find_ci(dir: &Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name))
        .map(|e| e.path())
}

/// Expands the paths given by the user into images and game folders. Folders that aren't a
/// game themselves are searched recursively; later parts of split images are skipped.
pub fn collect_inputs(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            walk(p, &mut out, 0);
        } else if p.is_file() && !is_later_part(p) {
            out.push(p.clone());
        }
    }
    out.dedup();
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if is_game_folder(dir) {
        out.push(dir.to_path_buf());
        return;
    }
    if depth > 8 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk(&p, out, depth + 1);
        } else if has_image_ext(&p) && !is_later_part(&p) {
            out.push(p);
        }
    }
}

pub fn has_image_ext(p: &Path) -> bool {
    p.extension().is_some_and(|e| EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// One file or directory of a game, as listed by [`Reader::entries`].
#[derive(Clone, Debug)]
pub struct Entry {
    /// Path inside the game with `/` separators and no leading slash.
    pub path: String,
    pub size: u64,
    pub is_dir: bool,
    /// Byte offset of the file's data in the image (images only).
    offset: u64,
}

impl Entry {
    /// Whether this is the dashboard update folder or something inside it.
    pub fn in_system_update(&self) -> bool {
        let top = self.path.split('/').next().unwrap_or("");
        crate::trim::is_system_update(top)
    }
}

/// Read access to a game's files, whatever it is stored in.
pub enum Reader {
    Image { dev: Box<ImageDevice>, volume: Box<VolumeDescriptor> },
    Folder(PathBuf),
}

impl Reader {
    pub fn open(path: &Path) -> Result<Self> {
        if path.is_dir() {
            return Ok(Self::Folder(path.to_path_buf()));
        }
        let (dev, _) = open_image(path)?;
        Self::from_device(dev)
    }

    pub fn from_device(mut dev: ImageDevice) -> Result<Self> {
        let volume = xdvdfs::read::read_volume(&mut dev).map_err(format)?;
        Ok(Self::Image { dev: Box::new(dev), volume: Box::new(volume) })
    }

    /// Every file and directory, parents before children.
    pub fn entries(&mut self) -> Result<Vec<Entry>> {
        let mut out = Vec::new();
        match self {
            Self::Image { dev, volume } => {
                let tree = volume.root_table.file_tree(dev.as_mut()).map_err(format)?;
                for (parent, node) in tree {
                    let name = node.name_str().map_err(format)?;
                    let path = format!("{}/{name}", parent).trim_start_matches('/').to_string();
                    let data = node.node.dirent;
                    let is_dir = data.is_directory();
                    let offset = if is_dir || data.data.size == 0 { 0 } else { data.data.offset(0).map_err(format)? };
                    out.push(Entry { path, size: if is_dir { 0 } else { data.data.size as u64 }, is_dir, offset });
                }
            }
            Self::Folder(root) => walk_folder(root, "", &mut out)?,
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    /// Every file and directory a conversion or extraction keeps.
    pub fn entries_kept(&mut self, strip_system_update: bool) -> Result<Vec<Entry>> {
        let mut entries = self.entries()?;
        if strip_system_update {
            entries.retain(|e| !e.in_system_update());
        }
        Ok(entries)
    }

    /// Reads `buf.len()` bytes of `entry` starting at `pos`.
    pub fn read(&mut self, entry: &Entry, pos: u64, buf: &mut [u8]) -> Result<()> {
        match self {
            Self::Image { dev, .. } => dev.read(entry.offset + pos, buf).map_err(Error::Io),
            Self::Folder(root) => {
                let p = root.join(&entry.path);
                let mut f = std::fs::File::open(&p).map_err(at(&p))?;
                f.seek(SeekFrom::Start(pos)).map_err(at(&p))?;
                f.read_exact(buf).map_err(at(&p))
            }
        }
    }

    /// Reads the first `max` bytes of `entry` (all of it, if it is smaller).
    pub fn read_head(&mut self, entry: &Entry, max: u64) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; entry.size.min(max) as usize];
        self.read(entry, 0, &mut buf)?;
        Ok(buf)
    }
}

/// The file at `path` among `entries`, ignoring case.
pub fn find_file<'a>(entries: &'a [Entry], path: &str) -> Option<&'a Entry> {
    entries.iter().find(|e| !e.is_dir && e.path.eq_ignore_ascii_case(path))
}

fn walk_folder(root: &Path, rel: &str, out: &mut Vec<Entry>) -> Result<()> {
    let dir = root.join(rel);
    for e in std::fs::read_dir(&dir).map_err(at(&dir))? {
        let e = e.map_err(at(&dir))?;
        let name = e.file_name().to_string_lossy().into_owned();
        let path = if rel.is_empty() { name } else { format!("{rel}/{name}") };
        let meta = e.metadata().map_err(at(&e.path()))?;
        if meta.is_dir() {
            out.push(Entry { path: path.clone(), size: 0, is_dir: true, offset: 0 });
            walk_folder(root, &path, out)?;
        } else {
            out.push(Entry { path, size: meta.len(), is_dir: false, offset: 0 });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_names() {
        assert_eq!(split_stem(Path::new("Halo.1.iso")).as_deref(), Some("Halo"));
        assert_eq!(split_stem(Path::new("Halo.iso")), None);
        assert!(is_later_part(Path::new("Halo.2.iso")));
        assert!(!is_later_part(Path::new("Halo.1.iso")));
        assert!(!is_later_part(Path::new("Halo 2.iso")));
    }
}
