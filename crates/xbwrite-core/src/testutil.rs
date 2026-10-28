//! Small fake games for tests.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use crate::convert::{convert, ConvertOptions};
use crate::scan::scan;

/// Writes an extracted game: an XBE with a title image, some data and a `$SystemUpdate` folder.
pub fn game_folder(dir: &Path, name: &str) -> PathBuf {
    let root = dir.join(name);
    let image = crate::xpr::tests::solid_dxt1(16, 0x07E0);
    let xbe = crate::xbe::tests::fake_xbe("Test Game", 0x4D53_0042, crate::xbe::REGION_NA, &image);
    write(&root.join("default.xbe"), &xbe);
    write(&root.join("media/big.bin"), &pattern(3 * 1024 * 1024 + 123, 7));
    write(&root.join("media/sub/small.txt"), b"hello xbox");
    write(&root.join("empty.dat"), b"");
    write(&root.join("$SystemUpdate/xboxupd.bin"), &pattern(200_000, 3));
    root
}

/// Packs `folder` into an XISO at `out`.
pub fn xiso(folder: &Path, out: &Path) {
    let game = scan(folder).unwrap();
    convert(&game, out, &ConvertOptions::default(), &AtomicBool::new(false), &mut |_| {}).unwrap();
}

/// Turns an XISO into a Redump-style XGD3 image: the file system behind a (blank) video
/// partition, followed by filler.
pub fn redump(xiso: &Path, out: &Path) {
    let data = std::fs::read(xiso).unwrap();
    let mut f = std::fs::File::create(out).unwrap();
    f.set_len(34_078_720).unwrap();
    std::io::Seek::seek(&mut f, std::io::SeekFrom::End(0)).unwrap();
    f.write_all(&data).unwrap();
    f.write_all(&pattern(5 * 1024 * 1024, 11)).unwrap();
}

pub fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect()
}

fn write(p: &Path, data: &[u8]) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, data).unwrap();
}
