//! The fonts the desktop app uses for proportional and monospace text, embedded
//! in the exports, with glyph-advance measurement for layout.
//!
//! Ubuntu Light is licensed under the Ubuntu Font Licence 1.0 (embedding in
//! documents is permitted) and Hack under the MIT licence with Bitstream Vera
//! terms; both texts are in `licenses/`.

use std::sync::{Arc, OnceLock};

pub const BODY_FONT: &[u8] = include_bytes!("../fonts/Ubuntu-Light.ttf");
pub const MONO_FONT: &[u8] = include_bytes!("../fonts/Hack-Regular.ttf");

/// Family names as they appear in the SVG text and the font database.
pub const BODY_FAMILY: &str = "Ubuntu";
pub const MONO_FAMILY: &str = "Hack";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontKind {
    Body,
    Mono,
}

struct Measure {
    face: ttf_parser::Face<'static>,
    units_per_em: f32,
}

fn measure(kind: FontKind) -> &'static Measure {
    static BODY: OnceLock<Measure> = OnceLock::new();
    static MONO: OnceLock<Measure> = OnceLock::new();
    let (cell, data) = match kind {
        FontKind::Body => (&BODY, BODY_FONT),
        FontKind::Mono => (&MONO, MONO_FONT),
    };
    cell.get_or_init(|| {
        let face = ttf_parser::Face::parse(data, 0).expect("embedded font parses");
        let units_per_em = f32::from(face.units_per_em());
        Measure { face, units_per_em }
    })
}

/// Width of `text` in points at `size`, from the font's horizontal advances.
pub fn text_width(kind: FontKind, text: &str, size: f32) -> f32 {
    let m = measure(kind);
    text.chars()
        .map(|c| {
            m.face
                .glyph_index(c)
                .and_then(|g| m.face.glyph_hor_advance(g))
                .map_or(0.5, |a| f32::from(a) / m.units_per_em)
        })
        .sum::<f32>()
        * size
}

/// Both fonts loaded for SVG text layout (no system fonts, so output does not
/// depend on the machine).
pub fn font_database() -> Arc<usvg::fontdb::Database> {
    let mut db = usvg::fontdb::Database::new();
    db.load_font_data(BODY_FONT.to_vec());
    db.load_font_data(MONO_FONT.to_vec());
    db.set_sans_serif_family(BODY_FAMILY);
    db.set_serif_family(BODY_FAMILY);
    db.set_monospace_family(MONO_FAMILY);
    Arc::new(db)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_scale_with_size_and_mono_is_fixed_pitch() {
        let w10 = text_width(FontKind::Body, "Magnet fluence", 10.0);
        let w20 = text_width(FontKind::Body, "Magnet fluence", 20.0);
        assert!(w10 > 40.0 && w10 < 90.0, "{w10}");
        assert!((w20 - 2.0 * w10).abs() < 1e-3);
        let a = text_width(FontKind::Mono, "iiii", 10.0);
        let b = text_width(FontKind::Mono, "WWWW", 10.0);
        assert!((a - b).abs() < 1e-3);
    }

    #[test]
    fn database_resolves_the_body_family() {
        let db = font_database();
        let query = usvg::fontdb::Query {
            families: &[usvg::fontdb::Family::Name(BODY_FAMILY)],
            ..Default::default()
        };
        assert!(db.query(&query).is_some());
        let mono = usvg::fontdb::Query {
            families: &[usvg::fontdb::Family::Name(MONO_FAMILY)],
            ..Default::default()
        };
        assert!(db.query(&mono).is_some());
    }
}
