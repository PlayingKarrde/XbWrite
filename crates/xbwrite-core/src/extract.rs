//! Extracting a game's files to a folder.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use crate::convert::{byte_meter, Phase, Progress, ProgressFn};
use crate::error::{at, Error, Result};
use crate::scan::Game;
use crate::source::{self, Reader};
use crate::util::{for_each_chunk, CHUNK};

#[derive(Clone, Debug, Default)]
pub struct ExtractOptions {
    /// Leave out the `$SystemUpdate` dashboard update folder.
    pub strip_system_update: bool,
    /// Replace `dest` if it is an earlier extraction.
    pub overwrite: bool,
}

/// Where the partial extraction is written before it is renamed to `dest`.
fn partial_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".partial");
    PathBuf::from(s)
}

/// Extracts every file of `game` into the folder `dest`, which is created. A cancelled or
/// failed extraction leaves nothing behind.
pub fn extract(
    game: &Game,
    dest: &Path,
    opts: &ExtractOptions,
    cancel: &AtomicBool,
    progress: ProgressFn,
) -> Result<PathBuf> {
    if !game.is_image() {
        return Err(Error::Unsupported("this game is already a folder".into()));
    }
    if dest.exists() {
        if !opts.overwrite {
            return Err(Error::Unsupported(format!("{} already exists", dest.display())));
        }
        if !source::is_game_folder(dest) {
            return Err(Error::Unsupported(format!(
                "{} exists and isn't an extracted game, so it won't be replaced",
                dest.display()
            )));
        }
    }
    let tmp = partial_path(dest);
    if tmp.exists() {
        std::fs::remove_dir_all(&tmp).map_err(at(&tmp))?;
    }
    let result = write_files(game, &tmp, opts, cancel, progress);
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    if dest.exists() {
        std::fs::remove_dir_all(dest).map_err(at(dest))?;
    }
    std::fs::rename(&tmp, dest).map_err(at(dest))?;
    Ok(dest.to_path_buf())
}

fn write_files(
    game: &Game,
    root: &Path,
    opts: &ExtractOptions,
    cancel: &AtomicBool,
    progress: ProgressFn,
) -> Result<()> {
    progress(Progress::Phase(Phase::Extracting));
    let mut reader = Reader::open(&game.source)?;
    let entries = reader.entries_kept(opts.strip_system_update)?;
    let mut advance = byte_meter(progress, entries.iter().map(|e| e.size).sum());

    std::fs::create_dir_all(root).map_err(at(root))?;
    let mut buf = vec![0u8; CHUNK];
    for e in &entries {
        let path = root.join(safe_relative(&e.path)?);
        if e.is_dir {
            std::fs::create_dir_all(&path).map_err(at(&path))?;
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(at(parent))?;
        }
        // Whole chunks go straight to the file, so it needs no buffer of its own.
        let mut out = std::fs::File::create(&path).map_err(at(&path))?;
        for_each_chunk(e.size, CHUNK, cancel, |pos, n| {
            reader.read(e, pos, &mut buf[..n])?;
            out.write_all(&buf[..n]).map_err(at(&path))?;
            advance(n as u64);
            Ok(())
        })?;
    }
    Ok(())
}

/// An image path as a relative host path, refusing anything that could escape the folder.
fn safe_relative(p: &str) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for part in p.split(['/', '\\']) {
        if part.is_empty() || part == "." || part == ".." || part.contains(':') {
            return Err(Error::Format(format!("unsafe file name in image: {p}")));
        }
        out.push(part);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::scan::scan;
    use crate::testutil;

    #[test]
    fn rejects_escaping_names() {
        assert!(safe_relative("media/intro.xmv").is_ok());
        assert!(safe_relative("../evil").is_err());
        assert!(safe_relative("C:/evil").is_err());
    }

    #[test]
    fn extracts_all_files() {
        let dir = tempfile::tempdir().unwrap();
        let folder = testutil::game_folder(dir.path(), "Src");
        let iso = dir.path().join("Game.iso");
        testutil::xiso(&folder, &iso);
        let game = scan(&iso).unwrap();

        let dest = dir.path().join("Game");
        let opts = ExtractOptions { strip_system_update: true, overwrite: false };
        extract(&game, &dest, &opts, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert_eq!(std::fs::read(dest.join("media/sub/small.txt")).unwrap(), b"hello xbox");
        assert_eq!(std::fs::read(dest.join("media/big.bin")).unwrap(), testutil::pattern(3 * 1024 * 1024 + 123, 7));
        assert!(!dest.join("$SystemUpdate").exists());
        assert!(!dir.path().join("Game.partial").exists());

        assert!(extract(&game, &dest, &opts, &AtomicBool::new(false), &mut |_| {}).is_err());
        let opts = ExtractOptions { overwrite: true, ..opts };
        extract(&game, &dest, &opts, &AtomicBool::new(false), &mut |_| {}).unwrap();
    }
}
