//! Reads what's needed to show a game before converting it: its title, artwork and how big
//! the trimmed image will be.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use xdvdfs::blockdev::NullBlockDevice;
use xdvdfs::write::fs::NullCopier;

use crate::convert::{build, with_source_fs};
use crate::error::{Error, Result};
use crate::source::{self, Layout, Reader};
use crate::xbe::{self, XbeInfo};
use crate::xpr::Icon;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    Image {
        layout: Layout,
        cso: bool,
    },
    /// An extracted game: a folder with `default.xbe` in it.
    Folder,
}

#[derive(Clone, Debug)]
pub struct Game {
    /// The image (its first part, when split) or game folder.
    pub source: PathBuf,
    /// Every file making up the image (just `source` unless it's split).
    pub parts: Vec<PathBuf>,
    pub kind: SourceKind,
    /// Name for output files: the source's name without extension or part number.
    pub name: String,
    pub xbe: Option<XbeInfo>,
    pub icon: Option<Icon>,
    /// Size of the source on disk.
    pub source_size: u64,
    /// Size of the trimmed XISO, with and without the dashboard update.
    pub xiso_size: u64,
    pub xiso_size_stripped: u64,
    /// Bytes of file data in `$SystemUpdate` (0 if the disc has none).
    pub system_update_size: u64,
    pub file_count: usize,
    /// Bytes of file data in the game, including `$SystemUpdate`.
    pub data_size: u64,
}

impl Game {
    /// The title from the XBE, falling back to the file name.
    pub fn title(&self) -> &str {
        match &self.xbe {
            Some(x) if !x.title.is_empty() => &x.title,
            _ => &self.name,
        }
    }

    pub fn is_image(&self) -> bool {
        matches!(self.kind, SourceKind::Image { .. })
    }

    /// Size of the converted XISO (before compression).
    pub fn output_size(&self, strip_system_update: bool) -> u64 {
        if strip_system_update {
            self.xiso_size_stripped
        } else {
            self.xiso_size
        }
    }

    /// Bytes of file data a conversion copies.
    pub fn copy_size(&self, strip_system_update: bool) -> u64 {
        if strip_system_update {
            self.data_size - self.system_update_size
        } else {
            self.data_size
        }
    }

    /// An XISO that converting wouldn't make any smaller.
    pub fn already_trimmed(&self, strip_system_update: bool) -> bool {
        matches!(self.kind, SourceKind::Image { layout: Layout::Xiso, cso: false })
            && self.source_size <= self.output_size(strip_system_update)
    }

    /// Short description of the source, e.g. "Redump XGD2", "XISO", "CSO", "Folder".
    pub fn kind_label(&self) -> String {
        match self.kind {
            SourceKind::Image { cso: true, .. } => "CSO".into(),
            SourceKind::Image { layout: Layout::Redump(x), .. } => format!("Redump {}", x.label()),
            SourceKind::Image { layout: Layout::Xiso, .. } => "XISO".into(),
            SourceKind::Folder => "Folder".into(),
        }
    }
}

/// Reads a game from an image or game folder.
pub fn scan(path: &Path) -> Result<Game> {
    let (kind, parts, name, mut reader) = if path.is_dir() {
        if !source::is_game_folder(path) {
            return Err(Error::NotXbox("this folder has no default.xbe".into()));
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        (SourceKind::Folder, vec![path.to_path_buf()], name, Reader::Folder(path.to_path_buf()))
    } else {
        // Opened once here for both the layout and the file list.
        let parts = source::image_parts(path);
        let cso = source::is_cso(path);
        let (dev, layout) = source::open_image_parts(&parts, cso)?;
        let stem = source::split_stem(path)
            .or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_default();
        let name = stem.strip_suffix(".xiso").map(str::to_string).unwrap_or(stem);
        (SourceKind::Image { layout, cso }, parts, name, Reader::from_device(dev)?)
    };

    let entries = reader.entries()?;
    let files = entries.iter().filter(|e| !e.is_dir);
    let data_size = files.clone().map(|e| e.size).sum();
    let system_update_size = files.clone().filter(|e| e.in_system_update()).map(|e| e.size).sum();
    let file_count = files.count();
    let source_size = match kind {
        SourceKind::Folder => data_size,
        SourceKind::Image { .. } => parts.iter().filter_map(|p| p.metadata().ok()).map(|m| m.len()).sum(),
    };

    let (xbe, icon) = read_xbe(&mut reader, &entries);
    let xiso_size = trimmed_size(path, false)?;
    let has_update = entries.iter().any(|e| !e.path.contains('/') && e.in_system_update());
    let xiso_size_stripped = if has_update { trimmed_size(path, true)? } else { xiso_size };

    Ok(Game {
        source: path.to_path_buf(),
        parts,
        kind,
        name,
        xbe,
        icon,
        source_size,
        xiso_size,
        xiso_size_stripped,
        system_update_size,
        file_count,
        data_size,
    })
}

/// Title metadata and image from `default.xbe`. A missing or odd XBE isn't an error: the game
/// is still converted, just shown by its file name.
fn read_xbe(reader: &mut Reader, entries: &[source::Entry]) -> (Option<XbeInfo>, Option<Icon>) {
    let Some(e) = source::find_file(entries, "default.xbe") else { return (None, None) };
    let Ok(first) = reader.read_head(e, 0x1000) else { return (None, None) };
    let Some(len) = xbe::headers_len(&first) else { return (None, None) };
    let Ok(headers) = reader.read_head(e, len.min(4 << 20) as u64) else { return (None, None) };
    let Some(h) = xbe::parse_headers(&headers) else { return (None, None) };
    let icon = h.image_section().and_then(|s| {
        let mut buf = vec![0u8; s.raw_size as usize];
        reader.read(e, s.raw_offset as u64, &mut buf).ok()?;
        xbe::decode_image(&buf)
    });
    (Some(h.info), icon)
}

/// Exact size of the XISO a conversion writes, worked out by laying the image out without
/// copying any data.
pub fn trimmed_size(path: &Path, strip_system_update: bool) -> Result<u64> {
    let cancel = AtomicBool::new(false);
    let mut out = NullBlockDevice::default();
    with_source_fs!(path, NullCopier<_>, |fs| build(fs, &mut out, strip_system_update, &cancel, &mut |_| {}))?;
    Ok(out.len_blocking())
}
