//! Decodes the XPR0 textures that XBEs use for their title image.
//!
//! Title images are almost always 128×128 DXT1. DXT3/5 and the common uncompressed formats
//! (swizzled or linear ARGB8888 and RGB565) are handled too; anything else is skipped.

use crate::util::u32_at;

/// Straight-alpha RGBA pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Icon {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

const MAGIC: &[u8; 4] = b"XPR0";

// Xbox D3D colour formats (bits 8..16 of the texture's Format field).
const FMT_R5G6B5: u32 = 0x05;
const FMT_A8R8G8B8: u32 = 0x06;
const FMT_X8R8G8B8: u32 = 0x07;
const FMT_DXT1: u32 = 0x0C;
const FMT_DXT3: u32 = 0x0E;
const FMT_DXT5: u32 = 0x0F;
const FMT_LIN_R5G6B5: u32 = 0x11;
const FMT_LIN_A8R8G8B8: u32 = 0x12;
const FMT_LIN_X8R8G8B8: u32 = 0x1E;

pub fn decode(data: &[u8]) -> Option<Icon> {
    if data.len() < 0x20 || &data[0..4] != MAGIC {
        return None;
    }
    let header_size = u32_at(data, 0x08)? as usize;
    let format = u32_at(data, 0x18)?;
    let size = u32_at(data, 0x1C)?;
    let pixels = data.get(header_size..)?;

    let color = (format >> 8) & 0xFF;
    let (width, height, pitch) = if size != 0 {
        let w = (size & 0xFFF) + 1;
        let h = ((size >> 12) & 0xFFF) + 1;
        (w, h, Some((((size >> 24) & 0xFF) + 1) * 64))
    } else {
        (1u32 << ((format >> 20) & 0xF), 1u32 << ((format >> 24) & 0xF), None)
    };
    if width == 0 || height == 0 || width > 1024 || height > 1024 {
        return None;
    }

    let rgba = match color {
        FMT_DXT1 => decode_dxt(pixels, width, height, Dxt::One)?,
        FMT_DXT3 => decode_dxt(pixels, width, height, Dxt::Three)?,
        FMT_DXT5 => decode_dxt(pixels, width, height, Dxt::Five)?,
        FMT_A8R8G8B8 | FMT_X8R8G8B8 => decode_plain(pixels, width, height, 4, true, None, color == FMT_X8R8G8B8)?,
        FMT_LIN_A8R8G8B8 | FMT_LIN_X8R8G8B8 => {
            decode_plain(pixels, width, height, 4, false, pitch, color == FMT_LIN_X8R8G8B8)?
        }
        FMT_R5G6B5 => decode_plain(pixels, width, height, 2, true, None, true)?,
        FMT_LIN_R5G6B5 => decode_plain(pixels, width, height, 2, false, pitch, true)?,
        _ => return None,
    };
    Some(Icon { width, height, rgba })
}

#[derive(Clone, Copy, PartialEq)]
enum Dxt {
    One,
    Three,
    Five,
}

fn decode_dxt(src: &[u8], w: u32, h: u32, kind: Dxt) -> Option<Vec<u8>> {
    let block_bytes = if kind == Dxt::One { 8 } else { 16 };
    let bw = w.div_ceil(4) as usize;
    let bh = h.div_ceil(4) as usize;
    if src.len() < bw * bh * block_bytes {
        return None;
    }
    let mut out = vec![0u8; (w * h * 4) as usize];
    for by in 0..bh {
        for bx in 0..bw {
            let block = &src[(by * bw + bx) * block_bytes..][..block_bytes];
            let (alpha, color) = match kind {
                Dxt::One => (None, block),
                Dxt::Three => (Some(explicit_alpha(&block[..8])), &block[8..]),
                Dxt::Five => (Some(interpolated_alpha(&block[..8])), &block[8..]),
            };
            let texels = color_block(color, kind == Dxt::One);
            for py in 0..4 {
                for px in 0..4 {
                    let (x, y) = (bx * 4 + px, by * 4 + py);
                    if x >= w as usize || y >= h as usize {
                        continue;
                    }
                    let i = py * 4 + px;
                    let mut c = texels[i];
                    if let Some(a) = &alpha {
                        c[3] = a[i];
                    }
                    out[(y * w as usize + x) * 4..][..4].copy_from_slice(&c);
                }
            }
        }
    }
    Some(out)
}

/// The 16 texels of a DXT colour block. `allow_transparent` is DXT1's three-colour mode.
fn color_block(b: &[u8], allow_transparent: bool) -> [[u8; 4]; 16] {
    let c0 = u16::from_le_bytes([b[0], b[1]]);
    let c1 = u16::from_le_bytes([b[2], b[3]]);
    let (a, z) = (rgb565(c0), rgb565(c1));
    let mix = |p: u16, q: u16, d: u16| -> [u8; 4] {
        let f = |i: usize| ((a[i] as u16 * p + z[i] as u16 * q) / d) as u8;
        [f(0), f(1), f(2), 255]
    };
    let palette = if c0 > c1 || !allow_transparent {
        [a, z, mix(2, 1, 3), mix(1, 2, 3)]
    } else {
        [a, z, mix(1, 1, 2), [0, 0, 0, 0]]
    };
    let bits = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    std::array::from_fn(|i| palette[((bits >> (i * 2)) & 3) as usize])
}

fn explicit_alpha(b: &[u8]) -> [u8; 16] {
    let bits = u64::from_le_bytes(b.try_into().unwrap());
    std::array::from_fn(|i| (((bits >> (i * 4)) & 0xF) as u8) * 17)
}

fn interpolated_alpha(b: &[u8]) -> [u8; 16] {
    let (a0, a1) = (b[0] as u32, b[1] as u32);
    let mut table = [0u32; 8];
    table[0] = a0;
    table[1] = a1;
    if a0 > a1 {
        for (i, t) in table.iter_mut().enumerate().skip(2) {
            let k = i as u32 - 1;
            *t = ((7 - k) * a0 + k * a1) / 7;
        }
    } else {
        for (i, t) in table.iter_mut().enumerate().take(6).skip(2) {
            let k = i as u32 - 1;
            *t = ((5 - k) * a0 + k * a1) / 5;
        }
        table[7] = 255;
    }
    let mut bits = 0u64;
    for (i, &v) in b[2..8].iter().enumerate() {
        bits |= (v as u64) << (i * 8);
    }
    std::array::from_fn(|i| table[((bits >> (i * 3)) & 7) as usize] as u8)
}

fn rgb565(c: u16) -> [u8; 4] {
    let r = ((c >> 11) & 0x1F) as u8;
    let g = ((c >> 5) & 0x3F) as u8;
    let b = (c & 0x1F) as u8;
    [(r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2), 255]
}

/// Uncompressed ARGB8888 / RGB565, swizzled or linear.
fn decode_plain(
    src: &[u8],
    w: u32,
    h: u32,
    bpp: usize,
    swizzled: bool,
    pitch: Option<u32>,
    opaque: bool,
) -> Option<Vec<u8>> {
    let row = pitch.map(|p| p as usize).unwrap_or(w as usize * bpp);
    let mut out = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let idx = if swizzled { swizzle(x, y, w, h) * bpp } else { y as usize * row + x as usize * bpp };
            let px = src.get(idx..idx + bpp)?;
            let c = if bpp == 4 {
                [px[2], px[1], px[0], if opaque { 255 } else { px[3] }]
            } else {
                rgb565(u16::from_le_bytes([px[0], px[1]]))
            };
            out[((y * w + x) * 4) as usize..][..4].copy_from_slice(&c);
        }
    }
    Some(out)
}

/// Index of texel (x, y) in a Morton-swizzled power-of-two texture.
fn swizzle(x: u32, y: u32, w: u32, h: u32) -> usize {
    let (mut idx, mut out_bit, mut bit) = (0usize, 0, 1u32);
    while bit < w || bit < h {
        if bit < w {
            idx |= ((x & bit != 0) as usize) << out_bit;
            out_bit += 1;
        }
        if bit < h {
            idx |= ((y & bit != 0) as usize) << out_bit;
            out_bit += 1;
        }
        bit <<= 1;
    }
    idx
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// An XPR0 holding a `size`×`size` DXT1 texture filled with one opaque colour.
    pub fn solid_dxt1(size: u32, rgb565: u16) -> Vec<u8> {
        let log = size.trailing_zeros();
        let mut b = vec![0u8; 0x40];
        b[0..4].copy_from_slice(MAGIC);
        b[8..12].copy_from_slice(&0x40u32.to_le_bytes());
        let format = (FMT_DXT1 << 8) | (log << 20) | (log << 24);
        b[0x18..0x1C].copy_from_slice(&format.to_le_bytes());
        for _ in 0..(size / 4) * (size / 4) {
            b.extend_from_slice(&rgb565.to_le_bytes());
            b.extend_from_slice(&0u16.to_le_bytes());
            b.extend_from_slice(&[0; 4]);
        }
        b
    }

    #[test]
    fn decodes_dxt1() {
        let icon = decode(&solid_dxt1(16, 0xF800)).unwrap();
        assert_eq!((icon.width, icon.height), (16, 16));
        assert_eq!(&icon.rgba[..4], &[255, 0, 0, 255]);
        assert_eq!(&icon.rgba[icon.rgba.len() - 4..], &[255, 0, 0, 255]);
    }

    #[test]
    fn swizzle_is_a_permutation() {
        let (w, h) = (8, 4);
        let mut seen = vec![false; (w * h) as usize];
        for y in 0..h {
            for x in 0..w {
                seen[swizzle(x, y, w, h)] = true;
            }
        }
        assert!(seen.iter().all(|&s| s));
    }

    #[test]
    fn rejects_other_data() {
        assert!(decode(b"not an xpr texture at all, nope").is_none());
    }
}
