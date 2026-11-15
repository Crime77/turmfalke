//! Grundtypen: Felder, Farben, Figuren, Zuege und Bitboard-Helfer.
//!
//! Felder: a1 = 0, b1 = 1, ..., h1 = 7, a2 = 8, ..., h8 = 63.

pub type Bitboard = u64;
pub type Square = usize;
pub type Color = usize;

pub const WHITE: Color = 0;
pub const BLACK: Color = 1;

pub const PAWN: usize = 0;
pub const KNIGHT: usize = 1;
pub const BISHOP: usize = 2;
pub const ROOK: usize = 3;
pub const QUEEN: usize = 4;
pub const KING: usize = 5;

/// Farbige Figur = farbe * 6 + figurentyp (0..12). 12 = leeres Feld.
pub const NO_PIECE: u8 = 12;
/// Kein Feld (z. B. kein en-passant-Feld).
pub const NO_SQ: Square = 64;

pub const CASTLE_WK: u8 = 1;
pub const CASTLE_WQ: u8 = 2;
pub const CASTLE_BK: u8 = 4;
pub const CASTLE_BQ: u8 = 8;

pub const A1: Square = 0;
pub const B1: Square = 1;
pub const C1: Square = 2;
pub const D1: Square = 3;
pub const E1: Square = 4;
pub const F1: Square = 5;
pub const G1: Square = 6;
pub const H1: Square = 7;
pub const A8: Square = 56;
pub const B8: Square = 57;
pub const C8: Square = 58;
pub const D8: Square = 59;
pub const E8: Square = 60;
pub const F8: Square = 61;
pub const G8: Square = 62;
pub const H8: Square = 63;

pub const FILE_A_BB: Bitboard = 0x0101_0101_0101_0101;
pub const RANK_1_BB: Bitboard = 0xFF;
pub const FILE_H_BB: Bitboard = FILE_A_BB << 7;
pub const RANK_8_BB: Bitboard = RANK_1_BB << 56;
/// Alle dunklen Felder (a1 ist dunkel).
pub const DARK_SQUARES: Bitboard = 0xAA55_AA55_AA55_AA55;

#[inline(always)]
pub const fn file_bb(f: usize) -> Bitboard {
    FILE_A_BB << f
}
#[inline(always)]
pub const fn rank_bb(r: usize) -> Bitboard {
    RANK_1_BB << (8 * r)
}
#[inline(always)]
pub const fn file_of(sq: Square) -> usize {
    sq & 7
}
#[inline(always)]
pub const fn rank_of(sq: Square) -> usize {
    sq >> 3
}
/// Reihe aus Sicht der Farbe c (0 = eigene Grundreihe).
#[inline(always)]
pub const fn relative_rank(c: Color, sq: Square) -> usize {
    if c == WHITE {
        sq >> 3
    } else {
        7 - (sq >> 3)
    }
}
#[inline(always)]
pub const fn sq_bb(sq: Square) -> Bitboard {
    1u64 << sq
}
#[inline(always)]
pub const fn make_piece(c: Color, pt: usize) -> u8 {
    (c * 6 + pt) as u8
}
#[inline(always)]
pub const fn piece_type(p: u8) -> usize {
    (p as usize) % 6
}
#[inline(always)]
pub const fn piece_color(p: u8) -> Color {
    (p as usize) / 6
}
#[inline(always)]
pub fn lsb(bb: Bitboard) -> Square {
    debug_assert!(bb != 0);
    bb.trailing_zeros() as Square
}
#[inline(always)]
pub fn msb(bb: Bitboard) -> Square {
    debug_assert!(bb != 0);
    63 - bb.leading_zeros() as Square
}
#[inline(always)]
pub fn pop_lsb(bb: &mut Bitboard) -> Square {
    debug_assert!(*bb != 0);
    let s = bb.trailing_zeros() as Square;
    *bb &= *bb - 1;
    s
}
#[inline(always)]
pub fn popcount(bb: Bitboard) -> i32 {
    bb.count_ones() as i32
}
#[inline(always)]
pub fn more_than_one(bb: Bitboard) -> bool {
    bb & bb.wrapping_sub(1) != 0
}

pub fn square_name(sq: Square) -> String {
    let f = (b'a' + file_of(sq) as u8) as char;
    let r = (b'1' + rank_of(sq) as u8) as char;
    format!("{}{}", f, r)
}

pub fn parse_square(s: &str) -> Option<Square> {
    let b = s.as_bytes();
    if b.len() != 2 {
        return None;
    }
    let f = b[0].wrapping_sub(b'a');
    let r = b[1].wrapping_sub(b'1');
    if f < 8 && r < 8 {
        Some(r as usize * 8 + f as usize)
    } else {
        None
    }
}

pub fn piece_to_char(p: u8) -> char {
    const CHARS: &[u8; 13] = b"PNBRQKpnbrqk.";
    CHARS[p as usize] as char
}

pub fn piece_from_char(c: char) -> Option<u8> {
    let idx = "PNBRQKpnbrqk".find(c)?;
    Some(idx as u8)
}

// ---------------------------------------------------------------------------
// Zuege: 16 Bit = von (6) | nach (6) | flags (4)
// ---------------------------------------------------------------------------

pub const QUIET: u16 = 0;
pub const DOUBLE_PUSH: u16 = 1;
pub const KING_CASTLE: u16 = 2;
pub const QUEEN_CASTLE: u16 = 3;
pub const CAPTURE: u16 = 4;
pub const EP_CAPTURE: u16 = 5;
/// Umwandlung: PROMO | (0 = Springer, 1 = Laeufer, 2 = Turm, 3 = Dame)
pub const PROMO: u16 = 8;
/// Umwandlung mit Schlagen: PROMO_CAPTURE | (0..3)
pub const PROMO_CAPTURE: u16 = 12;

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug, Hash)]
pub struct Move(pub u16);

impl Move {
    /// Kein Zug (a1a1 ist nie legal).
    pub const NONE: Move = Move(0);
    /// Nullzug (b1b1 ist nie legal).
    pub const NULL: Move = Move(1 | (1 << 6));

    #[inline(always)]
    pub const fn new(from: Square, to: Square, flags: u16) -> Move {
        Move(from as u16 | ((to as u16) << 6) | (flags << 12))
    }
    #[inline(always)]
    pub const fn from(self) -> Square {
        (self.0 & 63) as Square
    }
    #[inline(always)]
    pub const fn to(self) -> Square {
        ((self.0 >> 6) & 63) as Square
    }
    #[inline(always)]
    pub const fn flags(self) -> u16 {
        self.0 >> 12
    }
    #[inline(always)]
    pub const fn is_none(self) -> bool {
        self.0 == 0
    }
    #[inline(always)]
    pub const fn is_capture(self) -> bool {
        self.flags() & CAPTURE != 0
    }
    #[inline(always)]
    pub const fn is_promotion(self) -> bool {
        self.flags() & PROMO != 0
    }
    #[inline(always)]
    pub const fn is_castle(self) -> bool {
        self.flags() == KING_CASTLE || self.flags() == QUEEN_CASTLE
    }
    #[inline(always)]
    pub const fn is_ep(self) -> bool {
        self.flags() == EP_CAPTURE
    }
    /// Weder Schlagzug noch Umwandlung.
    #[inline(always)]
    pub const fn is_quiet(self) -> bool {
        self.flags() & (CAPTURE | PROMO) == 0
    }
    /// Figurentyp der Umwandlung (nur gueltig, wenn is_promotion()).
    #[inline(always)]
    pub const fn promotion_type(self) -> usize {
        KNIGHT + (self.flags() & 3) as usize
    }

    pub fn to_uci(self) -> String {
        if self.is_none() || self == Move::NULL {
            return "0000".to_string();
        }
        let mut s = square_name(self.from());
        s.push_str(&square_name(self.to()));
        if self.is_promotion() {
            s.push(match self.promotion_type() {
                KNIGHT => 'n',
                BISHOP => 'b',
                ROOK => 'r',
                _ => 'q',
            });
        }
        s
    }
}
