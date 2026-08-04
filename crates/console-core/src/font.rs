//! Built-in 4x6 pixel font.
//!
//! Each glyph is drawn on a 3x5 pixel grid inside a 4x6 cell (one blank column
//! to the right, one blank row below), so text advances 4px horizontally and
//! 6px vertically. Covers ASCII 32..=126; lowercase reuses the uppercase
//! glyphs, as permitted by the spec.
//!
//! Encoding: 15 bits per glyph, five rows of three pixels, top row in the most
//! significant bits, leftmost pixel of a row in that row's high bit.

/// Glyph cell width in pixels (3 drawn + 1 spacing).
pub const GLYPH_W: i32 = 4;
/// Glyph cell height in pixels (5 drawn + 1 spacing).
pub const GLYPH_H: i32 = 6;
/// Drawn pixels per glyph row.
const GLYPH_BITS_W: i32 = 3;
/// Drawn rows per glyph.
const GLYPH_BITS_H: i32 = 5;

/// First and last ASCII codepoints with a glyph.
pub const FIRST_CHAR: u32 = 32;
pub const LAST_CHAR: u32 = 126;

/// Logical layout size for `text`, in font cells. Every byte advances one
/// 4px cell (matching the renderer, including unsupported UTF-8 bytes), and
/// every newline starts another 6px line. The trailing spacing column/row is
/// part of the result so adjacent text blocks compose on the same grid.
pub fn text_size(text: &str) -> (i32, i32) {
    let mut width = 0i32;
    let mut line_width = 0i32;
    let mut height = GLYPH_H;
    for byte in text.bytes() {
        if byte == b'\n' {
            width = width.max(line_width);
            line_width = 0;
            height = height.saturating_add(GLYPH_H);
        } else {
            line_width = line_width.saturating_add(GLYPH_W);
        }
    }
    (width.max(line_width), height)
}

#[rustfmt::skip]
const GLYPHS: [u16; 95] = [
    0b000_000_000_000_000, // 32 ' '
    0b010_010_010_000_010, // 33 '!'
    0b101_101_000_000_000, // 34 '"'
    0b101_111_101_111_101, // 35 '#'
    0b010_111_110_111_010, // 36 '$'
    0b101_001_010_100_101, // 37 '%'
    0b010_101_010_101_011, // 38 '&'
    0b010_010_000_000_000, // 39 '\''
    0b001_010_010_010_001, // 40 '('
    0b100_010_010_010_100, // 41 ')'
    0b000_101_010_101_000, // 42 '*'
    0b000_010_111_010_000, // 43 '+'
    0b000_000_000_010_100, // 44 ','
    0b000_000_111_000_000, // 45 '-'
    0b000_000_000_000_010, // 46 '.'
    0b001_001_010_100_100, // 47 '/'
    0b111_101_101_101_111, // 48 '0'
    0b010_110_010_010_111, // 49 '1'
    0b111_001_111_100_111, // 50 '2'
    0b111_001_011_001_111, // 51 '3'
    0b101_101_111_001_001, // 52 '4'
    0b111_100_111_001_111, // 53 '5'
    0b111_100_111_101_111, // 54 '6'
    0b111_001_001_001_001, // 55 '7'
    0b111_101_111_101_111, // 56 '8'
    0b111_101_111_001_111, // 57 '9'
    0b000_010_000_010_000, // 58 ':'
    0b000_010_000_010_100, // 59 ';'
    0b001_010_100_010_001, // 60 '<'
    0b000_111_000_111_000, // 61 '='
    0b100_010_001_010_100, // 62 '>'
    0b111_001_011_000_010, // 63 '?'
    0b111_101_111_100_011, // 64 '@'
    0b010_101_111_101_101, // 65 'A'
    0b110_101_110_101_110, // 66 'B'
    0b011_100_100_100_011, // 67 'C'
    0b110_101_101_101_110, // 68 'D'
    0b111_100_110_100_111, // 69 'E'
    0b111_100_110_100_100, // 70 'F'
    0b011_100_101_101_011, // 71 'G'
    0b101_101_111_101_101, // 72 'H'
    0b111_010_010_010_111, // 73 'I'
    0b001_001_001_101_010, // 74 'J'
    0b101_101_110_101_101, // 75 'K'
    0b100_100_100_100_111, // 76 'L'
    0b101_111_111_101_101, // 77 'M'
    0b110_101_101_101_101, // 78 'N'
    0b111_101_101_101_111, // 79 'O'
    0b111_101_111_100_100, // 80 'P'
    0b111_101_101_111_001, // 81 'Q'
    0b110_101_110_101_101, // 82 'R'
    0b011_100_010_001_110, // 83 'S'
    0b111_010_010_010_010, // 84 'T'
    0b101_101_101_101_111, // 85 'U'
    0b101_101_101_101_010, // 86 'V'
    0b101_101_101_111_101, // 87 'W'
    0b101_101_010_101_101, // 88 'X'
    0b101_101_010_010_010, // 89 'Y'
    0b111_001_010_100_111, // 90 'Z'
    0b011_010_010_010_011, // 91 '['
    0b100_100_010_001_001, // 92 '\\'
    0b110_010_010_010_110, // 93 ']'
    0b010_101_000_000_000, // 94 '^'
    0b000_000_000_000_111, // 95 '_'
    0b100_010_000_000_000, // 96 '`'
    0b010_101_111_101_101, // 97 'a' (= 'A')
    0b110_101_110_101_110, // 98 'b'
    0b011_100_100_100_011, // 99 'c'
    0b110_101_101_101_110, // 100 'd'
    0b111_100_110_100_111, // 101 'e'
    0b111_100_110_100_100, // 102 'f'
    0b011_100_101_101_011, // 103 'g'
    0b101_101_111_101_101, // 104 'h'
    0b111_010_010_010_111, // 105 'i'
    0b001_001_001_101_010, // 106 'j'
    0b101_101_110_101_101, // 107 'k'
    0b100_100_100_100_111, // 108 'l'
    0b101_111_111_101_101, // 109 'm'
    0b110_101_101_101_101, // 110 'n'
    0b111_101_101_101_111, // 111 'o'
    0b111_101_111_100_100, // 112 'p'
    0b111_101_101_111_001, // 113 'q'
    0b110_101_110_101_101, // 114 'r'
    0b011_100_010_001_110, // 115 's'
    0b111_010_010_010_010, // 116 't'
    0b101_101_101_101_111, // 117 'u'
    0b101_101_101_101_010, // 118 'v'
    0b101_101_101_111_101, // 119 'w'
    0b101_101_010_101_101, // 120 'x'
    0b101_101_010_010_010, // 121 'y'
    0b111_001_010_100_111, // 122 'z'
    0b011_010_110_010_011, // 123 '{'
    0b010_010_010_010_010, // 124 '|'
    0b110_010_011_010_110, // 125 '}'
    0b000_011_111_110_000, // 126 '~'
];

/// Glyph bitmap for a byte, or `None` if it has no printable glyph.
pub fn glyph(ch: u8) -> Option<u16> {
    let c = u32::from(ch);
    if (FIRST_CHAR..=LAST_CHAR).contains(&c) {
        Some(GLYPHS[(c - FIRST_CHAR) as usize])
    } else {
        None
    }
}

/// True if the glyph has a pixel at `(col, row)` within its 3x5 grid.
pub fn pixel(bitmap: u16, col: i32, row: i32) -> bool {
    if !(0..GLYPH_BITS_W).contains(&col) || !(0..GLYPH_BITS_H).contains(&row) {
        return false;
    }
    let shift = (GLYPH_BITS_H - 1 - row) * GLYPH_BITS_W + (GLYPH_BITS_W - 1 - col);
    (bitmap >> shift) & 1 == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_printable_char_has_a_glyph() {
        for c in FIRST_CHAR..=LAST_CHAR {
            assert!(glyph(c as u8).is_some(), "missing glyph for {c}");
        }
        assert!(glyph(31).is_none());
        assert!(glyph(127).is_none());
    }

    #[test]
    fn only_space_is_blank() {
        for c in FIRST_CHAR..=LAST_CHAR {
            let g = glyph(c as u8).unwrap();
            if c == 32 {
                assert_eq!(g, 0);
            } else {
                assert_ne!(g, 0, "blank glyph for {c}");
            }
        }
    }

    #[test]
    fn lowercase_matches_uppercase() {
        for c in b'a'..=b'z' {
            assert_eq!(glyph(c), glyph(c.to_ascii_uppercase()));
        }
    }

    #[test]
    fn pixel_reads_top_left_first() {
        // 'A' top row is 0b010: only the middle column is lit.
        let a = glyph(b'A').unwrap();
        assert!(!pixel(a, 0, 0));
        assert!(pixel(a, 1, 0));
        assert!(!pixel(a, 2, 0));
        // Row 2 of 'A' is 0b111.
        assert!(pixel(a, 0, 2) && pixel(a, 1, 2) && pixel(a, 2, 2));
        // Out of range is never lit.
        assert!(!pixel(a, -1, 0));
        assert!(!pixel(a, 0, 5));
    }
}
