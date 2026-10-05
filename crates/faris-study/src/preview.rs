//! The optional `preview.png` thumbnail: a small picture of the 3D view the
//! author left open, for file managers and quick look. It carries no evidence,
//! so it is not hashed or listed in the manifest, and a reader that cannot use
//! it ignores it; it never refuses a study.

/// Longest side, in pixels, a written thumbnail may have.
pub const MAX_PREVIEW_SIDE: u32 = 512;
/// Largest thumbnail a writer stores and a reader will hand out.
pub const MAX_PREVIEW_BYTES_USED: u64 = 512 * 1024;

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// Width and height from the PNG signature and IHDR chunk, or `None` when the
/// bytes do not start like a PNG. The pixels are not decoded.
pub fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 33 || bytes[..8] != SIGNATURE {
        return None;
    }
    // First chunk: 4-byte length (13), type "IHDR", then width and height.
    if bytes[8..12] != [0, 0, 0, 13] || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((width, height))
}

/// True when a writer may store these bytes as the thumbnail.
pub(crate) fn is_storable(bytes: &[u8]) -> bool {
    bytes.len() as u64 <= MAX_PREVIEW_BYTES_USED
        && png_dimensions(bytes).is_some_and(|(w, h)| {
            (1..=MAX_PREVIEW_SIDE).contains(&w) && (1..=MAX_PREVIEW_SIDE).contains(&h)
        })
}

/// A thumbnail read from a study file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preview {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// What the file holds at `preview.png`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewStatus {
    Absent,
    Usable(Preview),
    /// Present but ignored, with the reason.
    Ignored {
        bytes: u64,
        reason: String,
    },
}
