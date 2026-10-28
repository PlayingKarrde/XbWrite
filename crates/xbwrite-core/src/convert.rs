//! Rebuilding a game as a trimmed XISO or a CSO.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use xdvdfs::blockdev::BlockDeviceWrite;
use xdvdfs::write::fs::{
    DefaultCopier, FilesystemCopier, FilesystemHierarchy, RWCopier, SectorLinearBlockDevice,
    SectorLinearBlockFilesystem, SectorLinearImage, XDVDFSFilesystem,
};
use xdvdfs::write::img::{create_xdvdfs_image, NoOpProgressVisitor};

use crate::error::{at, chain, Error, Result};
use crate::scan::Game;
use crate::source::{self, ImageDevice, Layout, Reader};
use crate::split::{part_name, SplitWriter};
use crate::trim::TrimFs;
use crate::util::{for_each_chunk, CHUNK};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Format {
    /// Uncompressed XISO, playable by xemu and every Xbox loader.
    #[default]
    Xiso,
    /// LZ4-compressed CSO, for loaders that read it (Cerbios, Stellar, XBMC4Gamers, …).
    Cso,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Xiso => "iso",
            Self::Cso => "cso",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Xiso => "XISO",
            Self::Cso => "CSO",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ConvertOptions {
    pub format: Format,
    /// Cut images over 4 GB into `.1.iso`, `.2.iso`, … parts for FAT32 drives.
    pub split: bool,
    /// Leave out the `$SystemUpdate` dashboard update folder.
    pub strip_system_update: bool,
    /// Read the new image back and compare every file with the source before keeping it.
    pub verify: bool,
    /// Delete the source image once the new one is written (and verified, if asked).
    pub delete_source: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Writing,
    Compressing,
    Verifying,
    Extracting,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Writing => "Writing",
            Self::Compressing => "Compressing",
            Self::Verifying => "Verifying",
            Self::Extracting => "Extracting",
        }
    }
}

#[derive(Clone, Debug)]
pub enum Progress {
    /// A new phase starts; the byte counts that follow are for it.
    Phase(Phase),
    Bytes {
        done: u64,
        total: u64,
    },
    Log(String),
}

pub type ProgressFn<'a> = &'a mut (dyn FnMut(Progress) + Send + Sync);

/// Reports a phase's `total` bytes of work, and returns a callback that adds `n` bytes done.
pub(crate) fn byte_meter(progress: ProgressFn<'_>, total: u64) -> impl FnMut(u64) + Send + Sync + '_ {
    let mut done = 0;
    progress(Progress::Bytes { done, total });
    move |n| {
        done += n;
        progress(Progress::Bytes { done, total });
    }
}

/// The XDVDFS file system of the image at `path`, as an xdvdfs source to build an image from.
pub(crate) fn image_fs<W, C>(path: &Path) -> Result<XDVDFSFilesystem<ImageDevice, W, C>>
where
    W: BlockDeviceWrite + ?Sized,
    C: RWCopier<ImageDevice, W>,
{
    let (dev, _) = source::open_image(path)?;
    XDVDFSFilesystem::new(dev).ok_or_else(|| Error::Format("can't read the file system".into()))
}

/// Evaluates `$body` with `$fs` bound to the game at `$path` as an xdvdfs source: the folder
/// itself, or the file system of the image, read with `$copier`. The two sources are different
/// types, so this is a macro rather than a function taking a closure.
macro_rules! with_source_fs {
    ($path:expr, $copier:ty, |$fs:ident| $body:expr) => {{
        let path: &std::path::Path = $path;
        if path.is_dir() {
            let $fs = xdvdfs::write::fs::StdFilesystem::create(path);
            $body
        } else {
            let $fs = $crate::convert::image_fs::<_, $copier>(path)?;
            $body
        }
    }};
}
pub(crate) use with_source_fs;

/// Where a converted game is written in `dir`: `<name>.iso` (or `.cso`), or `<name>.xiso.iso`
/// when that would overwrite the source and the source isn't being replaced.
pub fn output_path(game: &Game, dir: &Path, format: Format, replace_source: bool) -> PathBuf {
    let ext = format.extension();
    let plain = dir.join(format!("{}.{ext}", game.name));
    if replace_source && game.is_image() {
        return plain;
    }
    if clashes(&plain, &game.parts) {
        dir.join(format!("{}.xiso.{ext}", game.name))
    } else {
        plain
    }
}

/// Whether the output `p`, split or not, could land on one of `parts`.
fn clashes(p: &Path, parts: &[PathBuf]) -> bool {
    std::iter::once(p.to_path_buf())
        .chain((0..16).map(|i| part_name(p, i)))
        .any(|c| parts.iter().any(|s| same_path(s, &c)))
}

fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
    } else {
        a == b
    }
}

/// Converts `game` into the image `output` (split parts get numbered names next to it).
/// Returns the files written.
pub fn convert(
    game: &Game,
    output: &Path,
    opts: &ConvertOptions,
    cancel: &AtomicBool,
    progress: ProgressFn,
) -> Result<Vec<PathBuf>> {
    if let Some(dir) = output.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(at(dir))?;
    }
    let mut writer = SplitWriter::new(output, opts.split);
    let result = write_and_check(game, &mut writer, opts, cancel, progress);
    if let Err(e) = result {
        writer.discard();
        return Err(e);
    }

    // The final names are known now, so compare them exactly: `clashes` would also flag a
    // split source being replaced by an unsplit image of the same name.
    let finals = writer.final_paths();
    let overlaps = finals.iter().any(|f| game.parts.iter().any(|s| same_path(s, f)));
    if overlaps && !(opts.delete_source && game.is_image()) {
        writer.discard();
        return Err(Error::Unsupported(format!(
            "{} would overwrite the source. Choose another output folder.",
            output.display()
        )));
    }
    // Replacing the source in place: it has to go before the new files can take its name.
    if overlaps {
        delete_source(game, progress)?;
    }
    let written = writer.finish()?;
    if opts.delete_source && !overlaps {
        delete_source(game, progress)?;
    }
    Ok(written)
}

fn write_and_check(
    game: &Game,
    writer: &mut SplitWriter,
    opts: &ConvertOptions,
    cancel: &AtomicBool,
    progress: ProgressFn,
) -> Result<()> {
    let strip = opts.strip_system_update;
    match opts.format {
        Format::Xiso => {
            progress(Progress::Phase(Phase::Writing));
            let mut on_copy = byte_meter(progress, game.copy_size(strip));
            with_source_fs!(&game.source, DefaultCopier<_, _>, |fs| build(fs, writer, strip, cancel, &mut on_copy))?;
        }
        Format::Cso => {
            progress(Progress::Phase(Phase::Compressing));
            with_source_fs!(&game.source, DefaultCopier<_, _>, |fs| compress(fs, writer, strip, cancel, progress))?;
        }
    }
    let temps = writer.close()?;
    if opts.verify {
        verify(game, &temps, opts.format == Format::Cso, strip, cancel, progress)?;
    }
    Ok(())
}

fn map_err(e: &dyn std::error::Error, cancel: &AtomicBool) -> Error {
    if cancel.load(Ordering::Relaxed) {
        Error::Cancelled
    } else {
        Error::Format(chain(e))
    }
}

/// Writes the XDVDFS image of `inner` to `out`.
pub(crate) fn build<F, W>(
    inner: F,
    out: &mut W,
    strip: bool,
    cancel: &AtomicBool,
    on_copy: &mut (dyn FnMut(u64) + Send + Sync),
) -> Result<()>
where
    F: FilesystemHierarchy + FilesystemCopier<W>,
    W: BlockDeviceWrite + ?Sized,
{
    let mut fs = TrimFs { inner, skip_system_update: strip, cancel, on_copy };
    create_xdvdfs_image(&mut fs, out, NoOpProgressVisitor).map_err(|e| map_err(&e, cancel))
}

/// Lays out the XDVDFS image of `inner` and writes it to `out` as a CSO.
fn compress<F>(inner: F, out: &mut SplitWriter, strip: bool, cancel: &AtomicBool, progress: ProgressFn) -> Result<()>
where
    F: FilesystemHierarchy + FilesystemCopier<[u8]>,
{
    let mut nop = |_| {};
    let fs = TrimFs { inner, skip_system_update: strip, cancel, on_copy: &mut nop };
    let mut layout = SectorLinearBlockDevice::default();
    let mut fs = SectorLinearBlockFilesystem::new(fs);
    create_xdvdfs_image(&mut fs, &mut layout, NoOpProgressVisitor).map_err(|e| map_err(&e, cancel))?;

    let size = layout.size();
    let mut input = SectorLinearImage::new(&layout, &mut fs);
    let (mut sectors, mut finished) = (0u64, 0u64);
    let callback = |p| match p {
        ciso::write::ProgressInfo::SectorCount(n) => {
            sectors = n as u64;
            progress(Progress::Bytes { done: 0, total: size });
        }
        ciso::write::ProgressInfo::SectorFinished => {
            finished += 1;
            if finished % 512 == 0 {
                progress(Progress::Bytes { done: size * finished / sectors.max(1), total: size });
            }
        }
        _ => {}
    };
    ciso::write::write_ciso_image(&mut input, out, callback).map_err(|e| match e {
        _ if cancel.load(Ordering::Relaxed) => Error::Cancelled,
        ciso::write::CSOCreationError::WriteError(e) => Error::Io(e),
        e => Error::Format(e.to_string()),
    })?;
    progress(Progress::Bytes { done: size, total: size });
    Ok(())
}

/// Reads the image written to `temps` back and checks it holds exactly the source's files.
fn verify(
    game: &Game,
    temps: &[PathBuf],
    cso: bool,
    strip: bool,
    cancel: &AtomicBool,
    progress: ProgressFn,
) -> Result<()> {
    progress(Progress::Phase(Phase::Verifying));
    let mut src = Reader::open(&game.source)?;
    let (dev, layout) = source::open_image_parts(temps, cso)?;
    if layout != Layout::Xiso {
        return Err(Error::Verify("the new image's file system isn't at the start".into()));
    }
    let mut out = Reader::from_device(dev)?;

    let wanted = src.entries_kept(strip)?;
    let got = out.entries()?;
    if wanted.len() != got.len() {
        return Err(Error::Verify(format!("expected {} entries, found {}", wanted.len(), got.len())));
    }
    for (w, g) in wanted.iter().zip(&got) {
        if w.path != g.path || w.is_dir != g.is_dir || w.size != g.size {
            return Err(Error::Verify(format!("{} doesn't match the source", w.path)));
        }
    }

    let mut advance = byte_meter(progress, wanted.iter().map(|e| e.size).sum());
    let mut a = vec![0u8; CHUNK];
    let mut b = vec![0u8; CHUNK];
    for (w, g) in wanted.iter().zip(&got).filter(|(w, _)| !w.is_dir) {
        for_each_chunk(w.size, CHUNK, cancel, |pos, n| {
            src.read(w, pos, &mut a[..n])?;
            out.read(g, pos, &mut b[..n])?;
            if a[..n] != b[..n] {
                return Err(Error::Verify(format!("{} differs from the source", w.path)));
            }
            advance(n as u64);
            Ok(())
        })?;
    }
    Ok(())
}

fn delete_source(game: &Game, progress: ProgressFn) -> Result<()> {
    if !game.is_image() {
        progress(Progress::Log(format!("Kept the source folder {}", game.source.display())));
        return Ok(());
    }
    for p in &game.parts {
        std::fs::remove_file(p).map_err(at(p))?;
        progress(Progress::Log(format!("Deleted {}", p.display())));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::scan;
    use crate::testutil;

    fn run(game: &Game, out: &Path, opts: &ConvertOptions) -> Result<Vec<PathBuf>> {
        convert(game, out, opts, &AtomicBool::new(false), &mut |_| {})
    }

    fn names(path: &Path) -> Vec<String> {
        let mut r = Reader::open(path).unwrap();
        r.entries().unwrap().into_iter().map(|e| e.path).collect()
    }

    #[test]
    fn folder_to_xiso_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let folder = testutil::game_folder(dir.path(), "Game");
        let game = scan(&folder).unwrap();
        assert_eq!(game.title(), "Test Game");
        assert_eq!(game.xbe.as_ref().unwrap().title_id_label(), "MS-066");
        assert_eq!(game.icon.as_ref().map(|i| (i.width, i.height)), Some((16, 16)));
        assert_eq!(game.file_count, 5);
        assert_eq!(game.system_update_size, 200_000);

        let out = dir.path().join("out/Game.iso");
        let opts = ConvertOptions { verify: true, ..Default::default() };
        assert_eq!(run(&game, &out, &opts).unwrap(), vec![out.clone()]);
        assert_eq!(std::fs::metadata(&out).unwrap().len(), game.xiso_size);
        assert!(names(&out).contains(&"$SystemUpdate/xboxupd.bin".to_string()));
    }

    #[test]
    fn redump_is_trimmed_and_stripped() {
        let dir = tempfile::tempdir().unwrap();
        let folder = testutil::game_folder(dir.path(), "Game");
        testutil::xiso(&folder, &dir.path().join("plain.iso"));
        let iso = dir.path().join("Game (USA).iso");
        testutil::redump(&dir.path().join("plain.iso"), &iso);

        let game = scan(&iso).unwrap();
        assert_eq!(game.kind_label(), "Redump XGD3");
        assert_eq!(game.name, "Game (USA)");
        assert!(game.xiso_size_stripped < game.xiso_size);
        assert!(game.xiso_size < game.source_size / 4);

        let out = output_path(&game, dir.path(), Format::Xiso, false);
        assert_eq!(out, dir.path().join("Game (USA).xiso.iso"));
        let opts = ConvertOptions { verify: true, strip_system_update: true, ..Default::default() };
        run(&game, &out, &opts).unwrap();
        assert_eq!(std::fs::metadata(&out).unwrap().len(), game.xiso_size_stripped);
        assert!(!names(&out).iter().any(|n| n.starts_with("$SystemUpdate")));
        assert!(iso.exists());

        let again = scan(&out).unwrap();
        assert_eq!(again.kind_label(), "XISO");
        assert!(again.already_trimmed(true));
    }

    #[test]
    fn cso_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let folder = testutil::game_folder(dir.path(), "Game");
        let iso = dir.path().join("Game.iso");
        testutil::xiso(&folder, &iso);
        let game = scan(&iso).unwrap();

        let cso = output_path(&game, dir.path(), Format::Cso, false);
        assert_eq!(cso, dir.path().join("Game.cso"));
        let opts = ConvertOptions { format: Format::Cso, verify: true, ..Default::default() };
        run(&game, &cso, &opts).unwrap();
        assert!(std::fs::metadata(&cso).unwrap().len() < game.source_size);

        let packed = scan(&cso).unwrap();
        assert_eq!(packed.kind_label(), "CSO");
        assert_eq!(packed.title(), "Test Game");
        let back = dir.path().join("back/Game.iso");
        run(&packed, &back, &ConvertOptions { verify: true, ..Default::default() }).unwrap();
        assert_eq!(std::fs::read(&back).unwrap(), std::fs::read(&iso).unwrap());
    }

    #[test]
    fn replaces_source_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let folder = testutil::game_folder(dir.path(), "Game");
        testutil::xiso(&folder, &dir.path().join("plain.iso"));
        let iso = dir.path().join("Game.iso");
        testutil::redump(&dir.path().join("plain.iso"), &iso);
        let game = scan(&iso).unwrap();

        let out = output_path(&game, dir.path(), Format::Xiso, true);
        assert_eq!(out, iso);
        let opts = ConvertOptions { verify: true, delete_source: true, ..Default::default() };
        assert_eq!(run(&game, &out, &opts).unwrap(), vec![iso.clone()]);
        assert_eq!(std::fs::metadata(&iso).unwrap().len(), game.xiso_size);
        assert_eq!(scan(&iso).unwrap().kind_label(), "XISO");
    }

    #[test]
    fn refuses_to_overwrite_source_without_delete() {
        let dir = tempfile::tempdir().unwrap();
        let folder = testutil::game_folder(dir.path(), "Game");
        let iso = dir.path().join("Game.iso");
        testutil::xiso(&folder, &iso);
        let before = std::fs::read(&iso).unwrap();
        let game = scan(&iso).unwrap();
        assert!(run(&game, &iso, &ConvertOptions::default()).is_err());
        assert_eq!(std::fs::read(&iso).unwrap(), before);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2, "temp files left behind");
    }

    #[test]
    fn unsplit_output_next_to_split_source() {
        let dir = tempfile::tempdir().unwrap();
        let folder = testutil::game_folder(dir.path(), "Game");
        let plain = dir.path().join("plain.iso");
        testutil::xiso(&folder, &plain);
        let data = std::fs::read(&plain).unwrap();
        let (a, b) = data.split_at(data.len() / 2);
        std::fs::write(dir.path().join("Game.1.iso"), a).unwrap();
        std::fs::write(dir.path().join("Game.2.iso"), b).unwrap();
        let game = scan(&dir.path().join("Game.1.iso")).unwrap();
        assert_eq!(game.parts.len(), 2);

        let out = dir.path().join("Game.iso");
        assert_eq!(run(&game, &out, &ConvertOptions::default()).unwrap(), vec![out.clone()]);
        assert_eq!(std::fs::read(&out).unwrap(), data);
        assert!(game.parts.iter().all(|p| p.exists()));
    }

    #[test]
    fn cancel_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let folder = testutil::game_folder(dir.path(), "Game");
        let game = scan(&folder).unwrap();
        let out = dir.path().join("out/Game.iso");
        let cancel = AtomicBool::new(true);
        let r = convert(&game, &out, &ConvertOptions::default(), &cancel, &mut |_| {});
        assert!(matches!(r, Err(Error::Cancelled)), "{r:?}");
        assert_eq!(std::fs::read_dir(dir.path().join("out")).unwrap().count(), 0);
    }
}
