//! Reads the title metadata and title image out of an Xbox executable (`default.xbe`).
//!
//! Only the image header, the certificate and the section table are parsed: enough for the
//! game's name, title ID, region, version and the `$$XTIMAGE` artwork the dashboard shows.

use crate::util::{u16_at, u32_at};
use crate::xpr::{self, Icon};

const MAGIC: &[u8; 4] = b"XBEH";

/// Region flags from the certificate's `game_region` field.
pub const REGION_NA: u32 = 0x1;
pub const REGION_JAPAN: u32 = 0x2;
pub const REGION_WORLD: u32 = 0x4;
pub const REGION_DEBUG: u32 = 0x8000_0000;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct XbeInfo {
    pub title_id: u32,
    /// Title name from the certificate (up to 40 UTF-16 characters).
    pub title: String,
    pub region: u32,
    pub version: u32,
    pub disc_number: u32,
}

impl XbeInfo {
    /// The title ID the way the dashboard and title databases print it, e.g. `MS-004`.
    pub fn title_id_label(&self) -> String {
        let a = (self.title_id >> 24) as u8;
        let b = (self.title_id >> 16) as u8;
        let printable = |c: u8| c.is_ascii_alphanumeric();
        if printable(a) && printable(b) {
            format!("{}{}-{:03}", a as char, b as char, self.title_id & 0xFFFF)
        } else {
            format!("{:08X}", self.title_id)
        }
    }

    /// Short region codes, e.g. `["NA", "PAL"]`.
    pub fn region_codes(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.region & REGION_NA != 0 {
            out.push("NA");
        }
        if self.region & REGION_JAPAN != 0 {
            out.push("JP");
        }
        if self.region & REGION_WORLD != 0 {
            out.push("PAL");
        }
        if self.region & REGION_DEBUG != 0 {
            out.push("DEBUG");
        }
        out
    }
}

/// One entry of the XBE section table.
#[derive(Clone, Debug)]
pub struct Section {
    pub name: String,
    pub raw_offset: u32,
    pub raw_size: u32,
}

/// The parsed XBE headers.
#[derive(Clone, Debug)]
pub struct XbeHeaders {
    pub info: XbeInfo,
    pub sections: Vec<Section>,
}

/// Bytes of the XBE needed to parse its headers (`size_of_headers`), read from a valid header.
pub fn headers_len(first_page: &[u8]) -> Option<u32> {
    if first_page.len() < 0x10C || &first_page[0..4] != MAGIC {
        return None;
    }
    u32_at(first_page, 0x108)
}

/// Parses the headers of an XBE. `headers` must hold at least `size_of_headers` bytes.
pub fn parse_headers(headers: &[u8]) -> Option<XbeHeaders> {
    if headers.len() < 0x178 || &headers[0..4] != MAGIC {
        return None;
    }
    let base = u32_at(headers, 0x104)?;
    let cert_addr = u32_at(headers, 0x118)?;
    let section_count = u32_at(headers, 0x11C)?;
    let section_addr = u32_at(headers, 0x120)?;
    let at = |va: u32| va.checked_sub(base).map(|o| o as usize);

    let cert = at(cert_addr)?;
    let title_units: Vec<u16> =
        (0..40).filter_map(|i| u16_at(headers, cert + 0x0C + i * 2)).take_while(|&c| c != 0).collect();
    let info = XbeInfo {
        title_id: u32_at(headers, cert + 0x08)?,
        title: String::from_utf16_lossy(&title_units).trim().to_string(),
        region: u32_at(headers, cert + 0xA0).unwrap_or(0),
        disc_number: u32_at(headers, cert + 0xA8).unwrap_or(0),
        version: u32_at(headers, cert + 0xAC).unwrap_or(0),
    };

    let mut sections = Vec::new();
    let table = at(section_addr)?;
    for i in 0..section_count.min(256) as usize {
        let h = table + i * 0x38;
        let name_addr = u32_at(headers, h + 0x14)?;
        let name = at(name_addr).map(|o| c_string(headers, o)).unwrap_or_default();
        sections.push(Section { name, raw_offset: u32_at(headers, h + 0x0C)?, raw_size: u32_at(headers, h + 0x10)? });
    }
    Some(XbeHeaders { info, sections })
}

impl XbeHeaders {
    /// The section holding the title image: `$$XTIMAGE`, or the save image as a fallback.
    pub fn image_section(&self) -> Option<&Section> {
        ["$$XTIMAGE", "$$XSIMAGE"]
            .iter()
            .find_map(|n| self.sections.iter().find(|s| s.name == *n && s.raw_size > 0 && s.raw_size < 4 << 20))
    }
}

/// Decodes the title image held by an image section's raw bytes.
pub fn decode_image(section: &[u8]) -> Option<Icon> {
    xpr::decode(section)
}

fn c_string(b: &[u8], start: usize) -> String {
    let Some(tail) = b.get(start..) else { return String::new() };
    let end = tail.iter().position(|&c| c == 0).unwrap_or(tail.len()).min(64);
    String::from_utf8_lossy(&tail[..end]).into_owned()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Builds a minimal XBE: headers, a certificate and one `$$XTIMAGE` section holding `image`.
    pub fn fake_xbe(title: &str, title_id: u32, region: u32, image: &[u8]) -> Vec<u8> {
        let base = 0x10000u32;
        let mut b = vec![0u8; 0x1000];
        b[0..4].copy_from_slice(MAGIC);
        let put = |b: &mut Vec<u8>, o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut b, 0x104, base);
        put(&mut b, 0x108, 0x1000);
        put(&mut b, 0x118, base + 0x200); // certificate
        put(&mut b, 0x11C, 1);
        put(&mut b, 0x120, base + 0x400); // section table
        put(&mut b, 0x200 + 0x08, title_id);
        for (i, u) in title.encode_utf16().enumerate() {
            b[0x20C + i * 2..0x20E + i * 2].copy_from_slice(&u.to_le_bytes());
        }
        put(&mut b, 0x200 + 0xA0, region);
        put(&mut b, 0x200 + 0xAC, 3);
        put(&mut b, 0x400 + 0x0C, 0x1000);
        put(&mut b, 0x400 + 0x10, image.len() as u32);
        put(&mut b, 0x400 + 0x14, base + 0x500);
        b[0x500..0x509].copy_from_slice(b"$$XTIMAGE");
        b.extend_from_slice(image);
        b
    }

    #[test]
    fn parses_certificate_and_sections() {
        let xbe = fake_xbe("Halo", 0x4D53_0004, REGION_NA | REGION_WORLD, &[1, 2, 3]);
        assert_eq!(headers_len(&xbe), Some(0x1000));
        let h = parse_headers(&xbe).unwrap();
        assert_eq!(h.info.title, "Halo");
        assert_eq!(h.info.title_id_label(), "MS-004");
        assert_eq!(h.info.region_codes(), vec!["NA", "PAL"]);
        assert_eq!(h.info.version, 3);
        let s = h.image_section().unwrap();
        assert_eq!((s.raw_offset, s.raw_size), (0x1000, 3));
    }

    #[test]
    fn rejects_non_xbe() {
        assert!(parse_headers(&[0u8; 0x400]).is_none());
        assert!(headers_len(b"MZ\0\0").is_none());
    }
}
