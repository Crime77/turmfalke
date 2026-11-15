//! Statische Bewertung (handgeschrieben, getapert zwischen Mittel- und Endspiel).
//!
//! Bestandteile:
//! - Material + Feldtabellen (PeSTO-Basis, Bauerntabelle ohne Freibauernanteil)
//! - Mobilitaet je Figurentyp (Felder ohne eigene Figuren, nicht von
//!   gegnerischen Bauern angegriffen; gefesselte Figuren nur entlang der Fessel)
//! - Bauernstruktur: verbunden/Phalanx, isoliert, rueckstaendig, Doppelbauern,
//!   schwache Hebel
//! - Freibauern: Reihe, freier/sicherer Weg, Koenigsnaehe im Endspiel,
//!   Quadratregel (unaufhaltsame Freibauern im Bauernendspiel)
//! - Koenigssicherheit: Bauernschild, Bauernsturm, Angriffseinheiten auf die
//!   Koenigszone (nichtlinear), sichere Schachs, offene Flanken
//! - Figuren: Laeuferpaar, Tuerme auf (halb)offenen Linien, Turm/Dame auf der
//!   7. Reihe, Vorposten, Laeufer auf langen Diagonalen, eingesperrte Tuerme
//! - Drohungen: angegriffene/haengende Figuren, Bauernangriffe, Damenangriffe
//! - Raum, Initiative/Komplexitaet, Anzugsvorteil, Daempfung durch 50-Zuege-Zaehler
//! - Endspielwissen: KPK-Bitbasis, Mattfuehrung (KXK, KBNK), Remisskalierung
//!   (ungleichfarbige Laeufer, Leichtfigur ohne Bauern, falscher Laeufer,
//!   Randbauern-Festung, Turmendspiele)
//!
//! `evaluate` liefert Centipawns aus Sicht der Seite am Zug in
//! [-EVAL_LIMIT, EVAL_LIMIT]. Keine Heap-Allokation pro Aufruf, kein
//! veraenderlicher globaler Zustand (die KPK-Bitbasis wird einmalig in einem
//! LazyLock aufgebaut).

use crate::attacks::*;
use crate::position::Position;
use crate::types::*;
use std::ops::{Add, AddAssign, Mul, Neg, Sub, SubAssign};
use std::sync::LazyLock;

/// Betragsgrenze der statischen Bewertung (Mattwerte liegen darueber).
pub const EVAL_LIMIT: i32 = 10_000;
/// Zuschlag fuer theoretisch gewonnene Endspiele (KXK, KBNK, gewonnenes KPK).
pub const KNOWN_WIN: i32 = 2_000;
/// Anzugsvorteil fuer die Seite am Zug.
pub const TEMPO: i32 = 16;

/// Materialwerte Mittel-/Endspiel (PeSTO).
pub const MG_VALUE: [i32; 6] = [82, 337, 365, 477, 1025, 0];
pub const EG_VALUE: [i32; 6] = [94, 281, 297, 512, 936, 0];
const PHASE_WEIGHT: [i32; 6] = [0, 1, 1, 2, 4, 0];
const PHASE_MAX: i32 = 24;

/// Erzwingt den Aufbau der KPK-Bitbasis (sonst beim ersten KPK-Aufruf).
pub fn init() {
    LazyLock::force(&KPK);
}

// ---------------------------------------------------------------------------
// Wertepaar (Mittelspiel, Endspiel)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct S {
    mg: i32,
    eg: i32,
}

const fn s(mg: i32, eg: i32) -> S {
    S { mg, eg }
}

const S0: S = s(0, 0);

impl Add for S {
    type Output = S;
    #[inline(always)]
    fn add(self, o: S) -> S {
        s(self.mg + o.mg, self.eg + o.eg)
    }
}
impl Sub for S {
    type Output = S;
    #[inline(always)]
    fn sub(self, o: S) -> S {
        s(self.mg - o.mg, self.eg - o.eg)
    }
}
impl Neg for S {
    type Output = S;
    #[inline(always)]
    fn neg(self) -> S {
        s(-self.mg, -self.eg)
    }
}
impl AddAssign for S {
    #[inline(always)]
    fn add_assign(&mut self, o: S) {
        self.mg += o.mg;
        self.eg += o.eg;
    }
}
impl SubAssign for S {
    #[inline(always)]
    fn sub_assign(&mut self, o: S) {
        self.mg -= o.mg;
        self.eg -= o.eg;
    }
}
impl Mul<i32> for S {
    type Output = S;
    #[inline(always)]
    fn mul(self, k: i32) -> S {
        s(self.mg * k, self.eg * k)
    }
}

// ---------------------------------------------------------------------------
// Feldtabellen (Layout wie gedruckt: erste Zeile = 8. Reihe, a8 zuerst).
// Weisse Figur auf Feld sq (a1 = 0) -> Index sq ^ 56.
// ---------------------------------------------------------------------------

#[rustfmt::skip]
const MG_PAWN: [i32; 64] = [
      0,   0,   0,   0,   0,   0,   0,   0,
      7,  14,  23,  26,  43,  38,  23,   0,
     -3,   4,  13,  16,  33,  28,  13, -10,
    -14,  13,   6,  21,  23,  12,  17, -23,
    -27,  -2,  -5,  12,  17,   6,  10, -25,
    -26,  -4,  -4, -10,   3,   3,  33, -12,
    -35,  -1, -20, -23, -15,  24,  38, -22,
      0,   0,   0,   0,   0,   0,   0,   0,
];
#[rustfmt::skip]
const EG_PAWN: [i32; 64] = [
      0,   0,   0,   0,   0,   0,   0,   0,
     30,  30,  25,  20,  20,  20,  25,  30,
     20,  20,  16,  12,  10,  10,  16,  18,
     16,  12,   6,   2,  -1,   2,   8,   8,
     13,   9,  -3,  -7,  -7,  -8,   3,  -1,
      4,   7,  -6,   1,   0,  -5,  -1,  -8,
     13,   8,   8,  10,  13,   0,   2,  -7,
      0,   0,   0,   0,   0,   0,   0,   0,
];
#[rustfmt::skip]
const MG_KNIGHT: [i32; 64] = [
    -167, -89, -34, -49,  61, -97, -15, -107,
     -73, -41,  72,  36,  23,  62,   7,  -17,
     -47,  60,  37,  65,  84, 129,  73,   44,
      -9,  17,  19,  53,  37,  69,  18,   22,
     -13,   4,  16,  13,  28,  19,  21,   -8,
     -23,  -9,  12,  10,  19,  17,  25,  -16,
     -29, -53, -12,  -3,  -1,  18, -14,  -19,
    -105, -21, -58, -33, -17, -28, -19,  -23,
];
#[rustfmt::skip]
const EG_KNIGHT: [i32; 64] = [
    -58, -38, -13, -28, -31, -27, -63, -99,
    -25,  -8, -25,  -2,  -9, -25, -24, -52,
    -24, -20,  10,   9,  -1,  -9, -19, -41,
    -17,   3,  22,  22,  22,  11,   8, -18,
    -18,  -6,  16,  25,  16,  17,   4, -18,
    -23,  -3,  -1,  15,  10,  -3, -20, -22,
    -42, -20, -10,  -5,  -2, -20, -23, -44,
    -29, -51, -23, -15, -22, -18, -50, -64,
];
#[rustfmt::skip]
const MG_BISHOP: [i32; 64] = [
    -29,   4, -82, -37, -25, -42,   7,  -8,
    -26,  16, -18, -13,  30,  59,  18, -47,
    -16,  37,  43,  40,  35,  50,  37,  -2,
     -4,   5,  19,  50,  37,  37,   7,  -2,
     -6,  13,  13,  26,  34,  12,  10,   4,
      0,  15,  15,  15,  14,  27,  18,  10,
      4,  15,  16,   0,   7,  21,  33,   1,
    -33,  -3, -14, -21, -13, -12, -39, -21,
];
#[rustfmt::skip]
const EG_BISHOP: [i32; 64] = [
    -14, -21, -11,  -8,  -7,  -9, -17, -24,
     -8,  -4,   7, -12,  -3, -13,  -4, -14,
      2,  -8,   0,  -1,  -2,   6,   0,   4,
     -3,   9,  12,   9,  14,  10,   3,   2,
     -6,   3,  13,  19,   7,  10,  -3,  -9,
    -12,  -3,   8,  10,  13,   3,  -7, -15,
    -14, -18,  -7,  -1,   4,  -9, -15, -27,
    -23,  -9, -23,  -5,  -9, -16,  -5, -17,
];
#[rustfmt::skip]
const MG_ROOK: [i32; 64] = [
     32,  42,  32,  51,  63,   9,  31,  43,
     27,  32,  58,  62,  80,  67,  26,  44,
     -5,  19,  26,  36,  17,  45,  61,  16,
    -24, -11,   7,  26,  24,  35,  -8, -20,
    -36, -26, -12,  -1,   9,  -7,   6, -23,
    -45, -25, -16, -17,   3,   0,  -5, -33,
    -44, -16, -20,  -9,  -1,  11,  -6, -71,
    -19, -13,   1,  17,  16,   7, -37, -26,
];
#[rustfmt::skip]
const EG_ROOK: [i32; 64] = [
     13,  10,  18,  15,  12,  12,   8,   5,
     11,  13,  13,  11,  -3,   3,   8,   3,
      7,   7,   7,   5,   4,  -3,  -5,  -3,
      4,   3,  13,   1,   2,   1,  -1,   2,
      3,   5,   8,   4,  -5,  -6,  -8, -11,
     -4,   0,  -5,  -1,  -7, -12,  -8, -16,
     -6,  -6,   0,   2,  -9,  -9, -11,  -3,
     -9,   2,   3,  -1,  -5, -13,   4, -20,
];
#[rustfmt::skip]
const MG_QUEEN: [i32; 64] = [
    -28,   0,  29,  12,  59,  44,  43,  45,
    -24, -39,  -5,   1, -16,  57,  28,  54,
    -13, -17,   7,   8,  29,  56,  47,  57,
    -27, -27, -16, -16,  -1,  17,  -2,   1,
     -9, -26,  -9, -10,  -2,  -4,   3,  -3,
    -14,   2, -11,  -2,  -5,   2,  14,   5,
    -35,  -8,  11,   2,   8,  15,  -3,   1,
     -1, -18,  -9,  10, -15, -25, -31, -50,
];
#[rustfmt::skip]
const EG_QUEEN: [i32; 64] = [
     -9,  22,  22,  27,  27,  19,  10,  20,
    -17,  20,  32,  41,  58,  25,  30,   0,
    -20,   6,   9,  49,  47,  35,  19,   9,
      3,  22,  24,  45,  57,  40,  57,  36,
    -18,  28,  19,  47,  31,  34,  39,  23,
    -16, -27,  15,   6,   9,  17,  10,   5,
    -22, -23, -30, -16, -16, -23, -36, -32,
    -33, -28, -22, -43,  -5, -32, -20, -41,
];
#[rustfmt::skip]
const MG_KING: [i32; 64] = [
    -65,  23,  16, -15, -56, -34,   2,  13,
     29,  -1, -20,  -7,  -8,  -4, -38, -29,
     -9,  24,   2, -16, -20,   6,  22, -22,
    -17, -20, -12, -27, -30, -25, -14, -36,
    -49,  -1, -27, -39, -46, -44, -33, -51,
    -14, -14, -22, -46, -44, -30, -15, -27,
      1,   7,  -8, -64, -43, -16,   9,   8,
    -15,  36,  12, -54,   8, -28,  24,  14,
];
#[rustfmt::skip]
const EG_KING: [i32; 64] = [
    -74, -35, -18, -18, -11,  15,   4, -17,
    -12,  17,  14,  17,  17,  38,  23,  11,
     10,  17,  23,  15,  20,  45,  44,  13,
     -8,  22,  24,  27,  26,  33,  26,   3,
    -18,  -4,  21,  24,  27,  23,   9, -11,
    -19,  -3,  11,  21,  23,  16,   7,  -9,
    -27, -11,   4,  13,  14,   4,  -5, -17,
    -53, -34, -21, -11, -28, -14, -24, -43,
];

const MG_RAW: [[i32; 64]; 6] = [MG_PAWN, MG_KNIGHT, MG_BISHOP, MG_ROOK, MG_QUEEN, MG_KING];
const EG_RAW: [[i32; 64]; 6] = [EG_PAWN, EG_KNIGHT, EG_BISHOP, EG_ROOK, EG_QUEEN, EG_KING];

/// Material + Feldwert fuer Weiss, indiziert mit a1 = 0. Schwarz: Index sq ^ 56.
const PSQT: [[S; 64]; 6] = {
    let mut t = [[s(0, 0); 64]; 6];
    let mut pt = 0;
    while pt < 6 {
        let mut sq = 0;
        while sq < 64 {
            t[pt][sq] = s(MG_VALUE[pt] + MG_RAW[pt][sq ^ 56], EG_VALUE[pt] + EG_RAW[pt][sq ^ 56]);
            sq += 1;
        }
        pt += 1;
    }
    t
};

// ---------------------------------------------------------------------------
// Bitboard-Tabellen
// ---------------------------------------------------------------------------

/// Felder vor sq auf derselben Linie (aus Sicht der Farbe).
const FORWARD_FILE: [[Bitboard; 64]; 2] = {
    let mut t = [[0u64; 64]; 2];
    let mut sq = 0;
    while sq < 64 {
        let f = sq % 8;
        let r = sq / 8;
        let mut rr = r + 1;
        while rr < 8 {
            t[0][sq] |= 1u64 << (rr * 8 + f);
            rr += 1;
        }
        let mut rr = 0;
        while rr < r {
            t[1][sq] |= 1u64 << (rr * 8 + f);
            rr += 1;
        }
        sq += 1;
    }
    t
};

/// Felder vor sq auf derselben und den Nachbarlinien.
const PASSED_SPAN: [[Bitboard; 64]; 2] = {
    let mut t = [[0u64; 64]; 2];
    let mut c = 0;
    while c < 2 {
        let mut sq = 0;
        while sq < 64 {
            let ff = FORWARD_FILE[c][sq];
            let f = sq % 8;
            let mut span = ff;
            if f > 0 {
                span |= ff >> 1;
            }
            if f < 7 {
                span |= ff << 1;
            }
            t[c][sq] = span;
            sq += 1;
        }
        c += 1;
    }
    t
};

/// Alle Reihen strikt vor Reihe r (aus Sicht der Farbe).
const FORWARD_RANKS: [[Bitboard; 8]; 2] = {
    let mut t = [[0u64; 8]; 2];
    let mut r = 0;
    while r < 8 {
        let mut rr = 0;
        while rr < 8 {
            if rr > r {
                t[0][r] |= RANK_1_BB << (8 * rr);
            }
            if rr < r {
                t[1][r] |= RANK_1_BB << (8 * rr);
            }
            rr += 1;
        }
        r += 1;
    }
    t
};

const ADJACENT_FILES: [Bitboard; 8] = {
    let mut t = [0u64; 8];
    let mut f = 0;
    while f < 8 {
        if f > 0 {
            t[f] |= FILE_A_BB << (f - 1);
        }
        if f < 7 {
            t[f] |= FILE_A_BB << (f + 1);
        }
        f += 1;
    }
    t
};

/// Koenigsabstand (Chebyshev).
const DIST: [[u8; 64]; 64] = {
    let mut t = [[0u8; 64]; 64];
    let mut a = 0;
    while a < 64 {
        let mut b = 0;
        while b < 64 {
            let df = if a % 8 > b % 8 { a % 8 - b % 8 } else { b % 8 - a % 8 };
            let dr = if a / 8 > b / 8 { a / 8 - b / 8 } else { b / 8 - a / 8 };
            t[a][b] = if df > dr { df as u8 } else { dr as u8 };
            b += 1;
        }
        a += 1;
    }
    t
};

const QUEEN_SIDE: Bitboard = FILE_A_BB | (FILE_A_BB << 1) | (FILE_A_BB << 2) | (FILE_A_BB << 3);
const KING_SIDE: Bitboard = !QUEEN_SIDE;
const CENTER_FILES: Bitboard = (FILE_A_BB << 2) | (FILE_A_BB << 3) | (FILE_A_BB << 4) | (FILE_A_BB << 5);
const CENTER: Bitboard = (1u64 << 27) | (1u64 << 28) | (1u64 << 35) | (1u64 << 36);
const KING_FLANK: [Bitboard; 8] = [
    QUEEN_SIDE ^ (FILE_A_BB << 3),
    QUEEN_SIDE,
    QUEEN_SIDE,
    CENTER_FILES,
    CENTER_FILES,
    KING_SIDE,
    KING_SIDE,
    KING_SIDE ^ (FILE_A_BB << 4),
];
/// Eigene Spielhaelfte plus zwei Reihen (fuer Flankenangriffe).
const CAMP: [Bitboard; 2] = [
    !(rank_bb(5) | rank_bb(6) | rank_bb(7)),
    !(rank_bb(0) | rank_bb(1) | rank_bb(2)),
];
const OUTPOST_RANKS: [Bitboard; 2] = [
    rank_bb(3) | rank_bb(4) | rank_bb(5),
    rank_bb(4) | rank_bb(3) | rank_bb(2),
];
const SPACE_MASK: [Bitboard; 2] = [
    CENTER_FILES & (rank_bb(1) | rank_bb(2) | rank_bb(3)),
    CENTER_FILES & (rank_bb(6) | rank_bb(5) | rank_bb(4)),
];
const REL_RANK3: [Bitboard; 2] = [rank_bb(2), rank_bb(5)];
const REL_RANK7: [Bitboard; 2] = [rank_bb(6), rank_bb(1)];

// ---------------------------------------------------------------------------
// Bewertungsparameter (Centipawns)
// ---------------------------------------------------------------------------

#[rustfmt::skip]
const MOB_KNIGHT: [S; 9] = [
    s(-28, -38), s(-18, -26), s(-9, -13), s(-3, -5), s(2, 2), s(6, 7), s(10, 11), s(13, 13), s(15, 15),
];
#[rustfmt::skip]
const MOB_BISHOP: [S; 14] = [
    s(-24, -38), s(-16, -26), s(-8, -14), s(-2, -5), s(3, 2), s(7, 8), s(11, 13),
    s(14, 17), s(17, 21), s(19, 24), s(21, 27), s(23, 30), s(25, 32), s(26, 34),
];
#[rustfmt::skip]
const MOB_ROOK: [S; 15] = [
    s(-18, -45), s(-12, -30), s(-8, -17), s(-5, -8), s(-2, -1), s(0, 5), s(3, 10), s(5, 15),
    s(8, 20), s(10, 24), s(12, 28), s(14, 31), s(16, 34), s(17, 36), s(18, 38),
];
#[rustfmt::skip]
const MOB_QUEEN: [S; 28] = [
    s(-14, -32), s(-11, -25), s(-8, -18), s(-6, -13), s(-4, -9), s(-2, -5), s(0, -2),
    s(1, 0), s(3, 3), s(4, 5), s(6, 8), s(7, 10), s(8, 12), s(9, 14),
    s(10, 16), s(11, 18), s(12, 20), s(13, 22), s(14, 23), s(15, 24), s(16, 25),
    s(17, 26), s(18, 27), s(18, 28), s(19, 29), s(19, 30), s(20, 31), s(20, 32),
];

// Bauernstruktur
const ISOLATED: S = s(3, 7);
const BACKWARD: S = s(5, 11);
const DOUBLED: S = s(7, 25);
const WEAK_UNOPPOSED: S = s(8, 12);
const WEAK_LEVER: S = s(0, 25);
/// Basis fuer verbundene Bauern je relativer Reihe (interne Einheiten).
const CONNECTED: [i32; 8] = [0, 7, 8, 12, 29, 48, 86, 0];

// Freibauern
const PASSED_RANK: [S; 8] = [
    s(0, 0),
    s(6, 13),
    s(10, 15),
    s(9, 18),
    s(37, 32),
    s(101, 80),
    s(166, 117),
    s(0, 0),
];
const PASSED_FILE: S = s(7, 4);
const UNSTOPPABLE_PASSER: S = s(0, 400);

// Figuren
const BISHOP_PAIR: S = s(30, 55);
const OUTPOST_KNIGHT: S = s(36, 19);
const OUTPOST_BISHOP: S = s(18, 9);
const REACHABLE_OUTPOST: S = s(19, 5);
const MINOR_BEHIND_PAWN: S = s(11, 1);
const KING_PROTECTOR: S = s(4, 4);
const BISHOP_PAWNS: S = s(2, 3);
const LONG_DIAGONAL_BISHOP: S = s(27, 0);
const ROOK_OPEN_FILE: S = s(28, 11);
const ROOK_SEMI_OPEN_FILE: S = s(13, 2);
const ROOK_ON_QUEEN_FILE: S = s(4, 3);
const ROOK_ON_7TH: S = s(8, 18);
const QUEEN_ON_7TH: S = s(3, 10);
const TRAPPED_ROOK: S = s(12, 2);
const WEAK_QUEEN: S = s(29, 7);

// Koenig
#[rustfmt::skip]
const SHELTER: [[i32; 8]; 4] = [
    [ -4,  49,  56,  35,  23,  11,   15, 0],
    [-26,  37,  21, -29, -17,  -7,  -38, 0],
    [ -6,  45,  14,  -1,  19,   2,  -27, 0],
    [-23,  -8, -17, -31, -29, -40, -100, 0],
];
#[rustfmt::skip]
const UNBLOCKED_STORM: [[i32; 8]; 4] = [
    [ 51, -173, -100, 58, 30,  27,  30, 0],
    [ 28,  -15,   73, 27, 22,  -6,  12, 0],
    [ -4,   31,  101, 20, -1, -13,  -8, 0],
    [ -9,   -7,   61,  2,  7,  -9, -17, 0],
];
const BLOCKED_STORM: S = s(49, 37);
const KING_PAWN_DIST: i32 = 7;
const PAWNLESS_FLANK: S = s(10, 43);
const FLANK_ATTACKS: S = s(5, 0);
/// Gewichte je angreifendem Figurentyp (interne Gefahreneinheiten).
const KING_ATTACK_WEIGHT: [i32; 6] = [0, 81, 52, 44, 10, 0];
/// Sichere Schachs [einfach, mehrfach] je Figurentyp.
const SAFE_CHECK: [[i32; 2]; 6] = [[0, 0], [792, 1283], [645, 967], [1084, 1897], [772, 1119], [0, 0]];

// Drohungen
const THREAT_BY_MINOR: [S; 6] = [s(4, 14), s(35, 18), s(47, 25), s(54, 54), s(47, 72), s(0, 0)];
const THREAT_BY_ROOK: [S; 6] = [s(2, 20), s(23, 32), s(23, 27), s(0, 17), s(31, 17), s(0, 0)];
const THREAT_BY_KING: S = s(14, 40);
const HANGING: S = s(41, 16);
const RESTRICTED_PIECE: S = s(4, 3);
const THREAT_BY_SAFE_PAWN: S = s(104, 42);
const THREAT_BY_PAWN_PUSH: S = s(29, 18);
const KNIGHT_ON_QUEEN: S = s(10, 5);
const SLIDER_ON_QUEEN: S = s(35, 8);

/// Raumbewertung nur mit genug Figuren (Summe der Mittelspielwerte beider Seiten).
const SPACE_THRESHOLD: i32 = 4980;

// Mattfuehrung
const PUSH_CLOSE: [i32; 8] = [0, 0, 100, 80, 60, 40, 20, 10];
const EDGE_TABLE: [[i32; 4]; 4] = [[20, 30, 50, 70], [30, 40, 60, 80], [50, 60, 70, 90], [70, 80, 90, 100]];

// ---------------------------------------------------------------------------
// Bitboard-Helfer
// ---------------------------------------------------------------------------

#[inline(always)]
fn shift_up(c: Color, b: Bitboard) -> Bitboard {
    if c == WHITE {
        b << 8
    } else {
        b >> 8
    }
}

#[inline(always)]
fn shift_down(c: Color, b: Bitboard) -> Bitboard {
    if c == WHITE {
        b >> 8
    } else {
        b << 8
    }
}

/// Angriffe aller Bauern in b: (nach links, nach rechts).
#[inline(always)]
fn pawn_attacks_lr(c: Color, b: Bitboard) -> (Bitboard, Bitboard) {
    if c == WHITE {
        ((b & !FILE_A_BB) << 7, (b & !FILE_H_BB) << 9)
    } else {
        ((b & !FILE_A_BB) >> 9, (b & !FILE_H_BB) >> 7)
    }
}

#[inline(always)]
fn pawn_attacks_bb(c: Color, b: Bitboard) -> Bitboard {
    let (l, r) = pawn_attacks_lr(c, b);
    l | r
}

/// b und alle Felder davor (aus Sicht der Farbe c).
#[inline(always)]
fn fill_up(c: Color, mut b: Bitboard) -> Bitboard {
    if c == WHITE {
        b |= b << 8;
        b |= b << 16;
        b |= b << 32;
    } else {
        b |= b >> 8;
        b |= b >> 16;
        b |= b >> 32;
    }
    b
}

/// Feld direkt vor sq (nur fuer Bauern/Felder, die nicht auf der letzten Reihe stehen).
#[inline(always)]
fn forward_sq(c: Color, sq: Square) -> Square {
    if c == WHITE {
        sq + 8
    } else {
        sq - 8
    }
}

#[inline(always)]
fn dist(a: Square, b: Square) -> i32 {
    DIST[a][b] as i32
}

#[inline(always)]
fn is_dark(sq: Square) -> bool {
    DARK_SQUARES & sq_bb(sq) != 0
}

#[inline(always)]
fn push_to_edge(sq: Square) -> i32 {
    let f = file_of(sq);
    let r = rank_of(sq);
    let fd = if f <= 3 { 3 - f } else { f - 4 };
    let rd = if r <= 3 { 3 - r } else { r - 4 };
    EDGE_TABLE[rd][fd]
}

/// Figuren (beider Farben), die als einzige zwischen dem Koenig der Farbe c
/// und einem gegnerischen Langschrittler stehen.
fn slider_blockers(pos: &Position, c: Color, occ: Bitboard) -> Bitboard {
    let k = pos.king_sq(c);
    let them = c ^ 1;
    let rq = pos.pieces(them, ROOK) | pos.pieces(them, QUEEN);
    let bq = pos.pieces(them, BISHOP) | pos.pieces(them, QUEEN);
    let mut snipers = (rook_attacks(k, 0) & rq) | (bishop_attacks(k, 0) & bq);
    let occupancy = occ ^ snipers;
    let mut blockers = 0;
    while snipers != 0 {
        let sn = pop_lsb(&mut snipers);
        let b = between(k, sn) & occupancy;
        if b != 0 && !more_than_one(b) {
            blockers |= b;
        }
    }
    blockers
}

// ---------------------------------------------------------------------------
// Hauptbewertung
// ---------------------------------------------------------------------------

/// Bewertung in Centipawns aus Sicht der Seite am Zug.
pub fn evaluate(pos: &Position) -> i32 {
    if let Some(v) = special_endgame(pos) {
        return v.clamp(-EVAL_LIMIT, EVAL_LIMIT);
    }

    let mut ev = Ev::new(pos);
    let mut score = material_psqt(pos);

    score += ev.pawns(WHITE) - ev.pawns(BLACK);
    score += ev.pieces::<KNIGHT>(WHITE) - ev.pieces::<KNIGHT>(BLACK);
    score += ev.pieces::<BISHOP>(WHITE) - ev.pieces::<BISHOP>(BLACK);
    score += ev.pieces::<ROOK>(WHITE) - ev.pieces::<ROOK>(BLACK);
    score += ev.pieces::<QUEEN>(WHITE) - ev.pieces::<QUEEN>(BLACK);
    score += ev.mobility[WHITE] - ev.mobility[BLACK];
    score += ev.king(WHITE) - ev.king(BLACK);
    score += ev.threats(WHITE) - ev.threats(BLACK);
    score += ev.passers(WHITE) - ev.passers(BLACK);
    score += ev.unstoppable();
    score += ev.space(WHITE) - ev.space(BLACK);
    score += ev.initiative(score);

    // Phase
    let mut phase = 0;
    for pt in KNIGHT..KING {
        phase += PHASE_WEIGHT[pt] * popcount(pos.by_type[pt]);
    }
    let phase = phase.min(PHASE_MAX);

    // Endspielskalierung (nur Endspielanteil)
    let mut eg = score.eg;
    if eg != 0 {
        let strong = if eg > 0 { WHITE } else { BLACK };
        eg = eg * ev.eg_scale(strong) / 64;
    }
    let mut v = (score.mg * phase + eg * (PHASE_MAX - phase)) / PHASE_MAX;
    v += if pos.side == WHITE { TEMPO } else { -TEMPO };

    // Bauernlose Seite mit zu wenig Uebergewicht kann nicht gewinnen.
    if v > 0 {
        v = v * draw_factor(pos, &ev.npm, WHITE) / 64;
    } else if v < 0 {
        v = v * draw_factor(pos, &ev.npm, BLACK) / 64;
    }

    // Leichte Daempfung bei hohem 50-Zuege-Zaehler (foerdert Fortschritt).
    let hm = pos.halfmove.min(100) as i32;
    v = v * (200 - hm) / 200;

    let v = if pos.side == WHITE { v } else { -v };
    v.clamp(-EVAL_LIMIT, EVAL_LIMIT)
}

fn material_psqt(pos: &Position) -> S {
    let mut sc = S0;
    for pt in PAWN..=KING {
        let mut b = pos.pieces(WHITE, pt);
        while b != 0 {
            sc += PSQT[pt][pop_lsb(&mut b)];
        }
        let mut b = pos.pieces(BLACK, pt);
        while b != 0 {
            sc -= PSQT[pt][pop_lsb(&mut b) ^ 56];
        }
    }
    sc
}

/// Faktor (von 64), wenn Farbe c vorne liegt, aber keine Bauern hat.
fn draw_factor(pos: &Position, npm: &[i32; 2], c: Color) -> i32 {
    if pos.pieces(c, PAWN) != 0 {
        return 64;
    }
    let them = c ^ 1;
    let own = pos.by_color[c] & !pos.by_type[KING];
    let knights = pos.pieces(c, KNIGHT);
    let bishops = pos.pieces(c, BISHOP);
    // Genau zwei Springer: kein erzwingbares Matt (ein Springer faellt unten auf 0).
    if own == knights && popcount(knights) == 2 {
        return if pos.pieces(them, PAWN) == 0 { 0 } else { 16 };
    }
    // Nur gleichfarbige Laeufer (oder nackter Koenig).
    if own == bishops && (bishops & DARK_SQUARES == 0 || bishops & !DARK_SQUARES == 0) {
        return 0;
    }
    if npm[c] - npm[them] <= MG_VALUE[BISHOP] {
        return if npm[c] < MG_VALUE[ROOK] {
            0
        } else if npm[them] <= MG_VALUE[BISHOP] {
            4
        } else {
            14
        };
    }
    64
}

/// Zwischenergebnisse einer Bewertung (liegt komplett auf dem Stack).
struct Ev<'a> {
    pos: &'a Position,
    occ: Bitboard,
    ksq: [Square; 2],
    pawn_att: [Bitboard; 2],
    pawn_att2: [Bitboard; 2],
    /// Felder, die Bauern der Farbe irgendwann angreifen koennten.
    pawn_span: [Bitboard; 2],
    att: [Bitboard; 2],
    att2: [Bitboard; 2],
    att_by: [[Bitboard; 6]; 2],
    king_ring: [Bitboard; 2],
    /// Angreiferzahl / Gewichtssumme / Angriffe neben dem Koenig - indiziert
    /// mit der angreifenden Farbe.
    ka_count: [i32; 2],
    ka_weight: [i32; 2],
    ka_attacks: [i32; 2],
    mob_area: [Bitboard; 2],
    mobility: [S; 2],
    blockers: [Bitboard; 2],
    passed: [Bitboard; 2],
    npm: [i32; 2],
}

impl<'a> Ev<'a> {
    fn new(pos: &'a Position) -> Ev<'a> {
        let occ = pos.occupied();
        let ksq = [pos.king_sq(WHITE), pos.king_sq(BLACK)];
        let mut ev = Ev {
            pos,
            occ,
            ksq,
            pawn_att: [0; 2],
            pawn_att2: [0; 2],
            pawn_span: [0; 2],
            att: [0; 2],
            att2: [0; 2],
            att_by: [[0; 6]; 2],
            king_ring: [0; 2],
            ka_count: [0; 2],
            ka_weight: [0; 2],
            ka_attacks: [0; 2],
            mob_area: [0; 2],
            mobility: [S0; 2],
            blockers: [0; 2],
            passed: [0; 2],
            npm: [0; 2],
        };
        for c in [WHITE, BLACK] {
            let p = pos.pieces(c, PAWN);
            let (l, r) = pawn_attacks_lr(c, p);
            ev.pawn_att[c] = l | r;
            ev.pawn_att2[c] = l & r;
            ev.pawn_span[c] = pawn_attacks_bb(c, fill_up(c, p));
            let ka = king_attacks(ksq[c]);
            ev.att_by[c][KING] = ka;
            ev.att_by[c][PAWN] = ev.pawn_att[c];
            ev.att[c] = ka | ev.pawn_att[c];
            ev.att2[c] = (ka & ev.pawn_att[c]) | ev.pawn_att2[c];
            let mut npm = 0;
            for pt in KNIGHT..KING {
                npm += MG_VALUE[pt] * popcount(pos.pieces(c, pt));
            }
            ev.npm[c] = npm;
        }
        for c in [WHITE, BLACK] {
            let them = c ^ 1;
            let k = ksq[c];
            let center = rank_of(k).clamp(1, 6) * 8 + file_of(k).clamp(1, 6);
            ev.king_ring[c] = (king_attacks(center) | sq_bb(center)) & !ev.pawn_att2[c];
            ev.ka_count[them] = popcount(ev.king_ring[c] & ev.pawn_att[them]);
            ev.mob_area[c] = !pos.by_color[c] & !ev.pawn_att[them];
            ev.blockers[c] = slider_blockers(pos, c, occ);
        }
        ev
    }

    // ---------------------------------------------------------------- Bauern

    fn pawns(&mut self, us: Color) -> S {
        let pos = self.pos;
        let them = us ^ 1;
        let ours = pos.pieces(us, PAWN);
        let theirs = pos.pieces(them, PAWN);
        let mut score = S0;
        let mut b = ours;
        while b != 0 {
            let sq = pop_lsb(&mut b);
            let r = relative_rank(us, sq);
            let fwd = forward_sq(us, sq);
            let back = forward_sq(them, sq);

            let opposed = theirs & FORWARD_FILE[us][sq];
            let blocked = theirs & sq_bb(fwd);
            let stoppers = theirs & PASSED_SPAN[us][sq];
            let lever = theirs & pawn_attacks(us, sq);
            let lever_push = theirs & pawn_attacks(us, fwd);
            let doubled = ours & sq_bb(back);
            let neighbours = ours & ADJACENT_FILES[file_of(sq)];
            let phalanx = neighbours & rank_bb(rank_of(sq));
            let support = neighbours & rank_bb(rank_of(back));

            // Rueckstaendig: alle Nachbarn stehen weiter vorn, und das Feld
            // davor ist blockiert oder von einem Bauern bedroht.
            let backward = neighbours & FORWARD_RANKS[them][rank_of(fwd)] == 0 && (lever_push | blocked) != 0;

            // Freibauer (oder Kandidat, der sich durch Schlagen/Vorruecken befreit)
            let passed = (stoppers ^ lever) == 0
                || ((stoppers ^ lever_push) == 0 && popcount(phalanx) >= popcount(lever_push))
                || (stoppers == blocked
                    && r >= 4
                    && shift_up(us, support) & !(theirs | self.pawn_att2[them]) != 0);
            if passed {
                self.passed[us] |= sq_bb(sq);
            }

            if (support | phalanx) != 0 {
                let v = CONNECTED[r] * (2 + (phalanx != 0) as i32 - (opposed != 0) as i32)
                    + 17 * popcount(support);
                score += s(v * 6 / 10, v * (r as i32 - 2) * 45 / 400);
            } else if neighbours == 0 {
                score -= ISOLATED;
                if opposed == 0 {
                    score -= WEAK_UNOPPOSED;
                }
            } else if backward {
                score -= BACKWARD;
                if opposed == 0 {
                    score -= WEAK_UNOPPOSED;
                }
            }
            if support == 0 {
                if doubled != 0 {
                    score -= DOUBLED;
                }
                if more_than_one(lever) {
                    score -= WEAK_LEVER;
                }
            }
        }
        score
    }

    // --------------------------------------------------------------- Figuren

    #[inline(always)]
    fn pieces<const PT: usize>(&mut self, us: Color) -> S {
        let pos = self.pos;
        let them = us ^ 1;
        let mut score = S0;
        let mut bb = pos.pieces(us, PT);
        if bb == 0 {
            return score;
        }
        let outposts = OUTPOST_RANKS[us] & self.pawn_att[us] & !self.pawn_span[them];
        let pinned = self.blockers[us] & pos.by_color[us];
        while bb != 0 {
            let sq = pop_lsb(&mut bb);
            let mut b = match PT {
                KNIGHT => knight_attacks(sq),
                BISHOP => bishop_attacks(sq, self.occ ^ pos.pieces(us, QUEEN)),
                ROOK => rook_attacks(sq, self.occ ^ pos.pieces(us, QUEEN) ^ pos.pieces(us, ROOK)),
                _ => queen_attacks(sq, self.occ),
            };
            if pinned & sq_bb(sq) != 0 {
                b &= line(self.ksq[us], sq);
            }
            self.att2[us] |= self.att[us] & b;
            self.att[us] |= b;
            self.att_by[us][PT] |= b;

            if b & self.king_ring[them] != 0 {
                self.ka_count[us] += 1;
                self.ka_weight[us] += KING_ATTACK_WEIGHT[PT];
                self.ka_attacks[us] += popcount(b & self.att_by[them][KING]);
            }

            let mob = popcount(b & self.mob_area[us]) as usize;
            self.mobility[us] += match PT {
                KNIGHT => MOB_KNIGHT[mob.min(8)],
                BISHOP => MOB_BISHOP[mob.min(13)],
                ROOK => MOB_ROOK[mob.min(14)],
                _ => MOB_QUEEN[mob.min(27)],
            };

            if PT == KNIGHT || PT == BISHOP {
                if outposts & sq_bb(sq) != 0 {
                    score += if PT == KNIGHT { OUTPOST_KNIGHT } else { OUTPOST_BISHOP };
                } else if PT == KNIGHT && outposts & b & !pos.by_color[us] != 0 {
                    score += REACHABLE_OUTPOST;
                }
                if shift_down(us, pos.by_type[PAWN]) & sq_bb(sq) != 0 {
                    score += MINOR_BEHIND_PAWN;
                }
                score -= KING_PROTECTOR * dist(sq, self.ksq[us]);

                if PT == BISHOP {
                    let same_color = if is_dark(sq) { DARK_SQUARES } else { !DARK_SQUARES };
                    let own_pawns = pos.pieces(us, PAWN);
                    let blocked = own_pawns & shift_down(us, self.occ);
                    let k = (self.pawn_att[us] & sq_bb(sq) == 0) as i32 + popcount(blocked & CENTER_FILES);
                    score -= BISHOP_PAWNS * (popcount(own_pawns & same_color) * k);
                    if more_than_one(bishop_attacks(sq, pos.by_type[PAWN]) & CENTER) {
                        score += LONG_DIAGONAL_BISHOP;
                    }
                }
            }

            if PT == ROOK {
                let fb = file_bb(file_of(sq));
                if pos.pieces(us, PAWN) & fb == 0 {
                    score += if pos.pieces(them, PAWN) & fb == 0 { ROOK_OPEN_FILE } else { ROOK_SEMI_OPEN_FILE };
                } else if mob <= 3 {
                    let kf = file_of(self.ksq[us]);
                    if (kf < 4) == (file_of(sq) < kf) {
                        let rights = if us == WHITE { CASTLE_WK | CASTLE_WQ } else { CASTLE_BK | CASTLE_BQ };
                        let mult = if pos.castling & rights == 0 { 2 } else { 1 };
                        score -= TRAPPED_ROOK * ((4 - mob as i32) * mult);
                    }
                }
                if pos.by_type[QUEEN] & fb != 0 {
                    score += ROOK_ON_QUEEN_FILE;
                }
            }

            if PT == ROOK || PT == QUEEN {
                if relative_rank(us, sq) == 6
                    && (relative_rank(us, self.ksq[them]) == 7 || pos.pieces(them, PAWN) & REL_RANK7[us] != 0)
                {
                    score += if PT == ROOK { ROOK_ON_7TH } else { QUEEN_ON_7TH };
                }
            }

            if PT == QUEEN {
                // Fesselung/Abzugsangriff auf die Dame durch Turm oder Laeufer.
                let mut snipers = (rook_attacks(sq, 0) & pos.pieces(them, ROOK))
                    | (bishop_attacks(sq, 0) & pos.pieces(them, BISHOP));
                let occupancy = self.occ ^ snipers;
                while snipers != 0 {
                    let sn = pop_lsb(&mut snipers);
                    let bt = between(sq, sn) & occupancy;
                    if bt != 0 && !more_than_one(bt) {
                        score -= WEAK_QUEEN;
                        break;
                    }
                }
            }
        }
        if PT == BISHOP {
            let b = pos.pieces(us, BISHOP);
            if b & DARK_SQUARES != 0 && b & !DARK_SQUARES != 0 {
                score += BISHOP_PAIR;
            }
        }
        score
    }

    // ---------------------------------------------------------------- Koenig

    /// Bauernschild und -sturm fuer einen Koenig der Farbe us auf ksq.
    fn shelter_storm(&self, us: Color, ksq: Square) -> S {
        let pos = self.pos;
        let them = us ^ 1;
        let b = pos.by_type[PAWN] & !FORWARD_RANKS[them][rank_of(ksq)];
        let ours = b & pos.by_color[us];
        let theirs = b & pos.by_color[them];
        let mut bonus = s(3, 2);
        let center = file_of(ksq).clamp(1, 6);
        for f in (center - 1)..=(center + 1) {
            let fb = file_bb(f);
            let o = ours & fb;
            let our_rank = if o != 0 { relative_rank(us, if us == WHITE { lsb(o) } else { msb(o) }) } else { 0 };
            let t = theirs & fb;
            let their_rank = if t != 0 { relative_rank(us, if us == WHITE { lsb(t) } else { msb(t) }) } else { 0 };
            let d = f.min(7 - f);
            bonus.mg += SHELTER[d][our_rank];
            if our_rank != 0 && our_rank + 1 == their_rank {
                if their_rank == 2 {
                    bonus -= BLOCKED_STORM;
                }
            } else {
                bonus.mg -= UNBLOCKED_STORM[d][their_rank];
            }
        }
        bonus
    }

    fn king(&self, us: Color) -> S {
        let pos = self.pos;
        let them = us ^ 1;
        let ksq = self.ksq[us];

        // Bauernschild (ggf. nach moeglicher Rochade)
        let mut shelter = self.shelter_storm(us, ksq);
        let (rk, rq, g1, c1) =
            if us == WHITE { (CASTLE_WK, CASTLE_WQ, G1, C1) } else { (CASTLE_BK, CASTLE_BQ, G8, C8) };
        if pos.castling & rk != 0 {
            let alt = self.shelter_storm(us, g1);
            if alt.mg > shelter.mg {
                shelter = alt;
            }
        }
        if pos.castling & rq != 0 {
            let alt = self.shelter_storm(us, c1);
            if alt.mg > shelter.mg {
                shelter = alt;
            }
        }

        // Im Endspiel den Koenig in die Naehe der eigenen Bauern
        let own_pawns = pos.pieces(us, PAWN);
        let mut min_dist = 6;
        if own_pawns & king_attacks(ksq) != 0 {
            min_dist = 1;
        } else {
            let mut p = own_pawns;
            while p != 0 {
                min_dist = min_dist.min(dist(ksq, pop_lsb(&mut p)));
            }
        }
        let mut score = shelter - s(0, KING_PAWN_DIST * min_dist);

        // Angriffsgefahr
        let weak = self.att[them]
            & !self.att2[us]
            & (!self.att[us] | self.att_by[us][KING] | self.att_by[us][QUEEN]);
        let safe = !pos.by_color[them] & (!self.att[us] | (weak & self.att2[them]));
        let occ_nq = self.occ ^ pos.pieces(us, QUEEN);
        let b1 = rook_attacks(ksq, occ_nq);
        let b2 = bishop_attacks(ksq, occ_nq);

        let mut kd = 0i32;
        let mut unsafe_checks = 0u64;

        let rook_checks = b1 & safe & self.att_by[them][ROOK];
        if rook_checks != 0 {
            kd += SAFE_CHECK[ROOK][more_than_one(rook_checks) as usize];
        } else {
            unsafe_checks |= b1 & self.att_by[them][ROOK];
        }
        let queen_checks = (b1 | b2) & self.att_by[them][QUEEN] & safe & !self.att_by[us][QUEEN] & !rook_checks;
        if queen_checks != 0 {
            kd += SAFE_CHECK[QUEEN][more_than_one(queen_checks) as usize];
        }
        let bishop_checks = b2 & self.att_by[them][BISHOP] & safe & !queen_checks;
        if bishop_checks != 0 {
            kd += SAFE_CHECK[BISHOP][more_than_one(bishop_checks) as usize];
        } else {
            unsafe_checks |= b2 & self.att_by[them][BISHOP];
        }
        let knight_checks = knight_attacks(ksq) & self.att_by[them][KNIGHT];
        if knight_checks & safe != 0 {
            kd += SAFE_CHECK[KNIGHT][more_than_one(knight_checks & safe) as usize];
        } else {
            unsafe_checks |= knight_checks;
        }

        let flank = KING_FLANK[file_of(ksq)] & CAMP[us];
        let fa = self.att[them] & flank;
        let flank_attack = popcount(fa) + popcount(fa & self.att2[them]);
        let flank_defense = popcount(self.att[us] & flank);

        kd += self.ka_count[them] * self.ka_weight[them]
            + 185 * popcount(self.king_ring[us] & weak)
            + 148 * popcount(unsafe_checks)
            + 98 * popcount(self.blockers[us])
            + 69 * self.ka_attacks[them]
            + 3 * flank_attack * flank_attack / 8
            + (self.mobility[them].mg - self.mobility[us].mg) * 16 / 10
            - 873 * (pos.pieces(them, QUEEN) == 0) as i32
            - 100 * (self.att_by[us][KNIGHT] & self.att_by[us][KING] != 0) as i32
            - 6 * (shelter.mg * 16 / 10) / 8
            - 4 * flank_defense
            + 37;

        if kd > 100 {
            let kd = kd.min(6000);
            score -= s(kd * kd / 6827, kd / 36);
        }

        if pos.by_type[PAWN] & KING_FLANK[file_of(ksq)] == 0 {
            score -= PAWNLESS_FLANK;
        }
        score -= FLANK_ATTACKS * flank_attack;
        score
    }

    // ------------------------------------------------------------- Drohungen

    fn threats(&self, us: Color) -> S {
        let pos = self.pos;
        let them = us ^ 1;
        let mut score = S0;
        let enemies = pos.by_color[them] & !pos.by_type[KING];
        let non_pawn_enemies = enemies & !pos.by_type[PAWN];
        let strongly_protected = self.pawn_att[them] | (self.att2[them] & !self.att2[us]);
        let defended = non_pawn_enemies & strongly_protected;
        let weak = enemies & !strongly_protected & self.att[us];

        if defended | weak != 0 {
            let mut b = (defended | weak) & (self.att_by[us][KNIGHT] | self.att_by[us][BISHOP]);
            while b != 0 {
                let sq = pop_lsb(&mut b);
                score += THREAT_BY_MINOR[piece_type(pos.board[sq])];
            }
            let mut b = weak & self.att_by[us][ROOK];
            while b != 0 {
                let sq = pop_lsb(&mut b);
                score += THREAT_BY_ROOK[piece_type(pos.board[sq])];
            }
            if weak & self.att_by[us][KING] != 0 {
                score += THREAT_BY_KING;
            }
            let b = !self.att[them] | (non_pawn_enemies & self.att2[us]);
            score += HANGING * popcount(weak & b);
        }

        let b = self.att[them] & !strongly_protected & self.att[us];
        score += RESTRICTED_PIECE * popcount(b);

        let safe = !self.att[them] | self.att[us];
        let b = pawn_attacks_bb(us, pos.pieces(us, PAWN) & safe) & non_pawn_enemies;
        score += THREAT_BY_SAFE_PAWN * popcount(b);

        let mut b = shift_up(us, pos.pieces(us, PAWN)) & !self.occ;
        b |= shift_up(us, b & REL_RANK3[us]) & !self.occ;
        b &= !self.pawn_att[them] & safe;
        let b = pawn_attacks_bb(us, b) & non_pawn_enemies;
        score += THREAT_BY_PAWN_PUSH * popcount(b);

        let q = pos.pieces(them, QUEEN);
        if q != 0 && !more_than_one(q) {
            let qs = lsb(q);
            let safe = self.mob_area[us] & !strongly_protected;
            let b = self.att_by[us][KNIGHT] & knight_attacks(qs);
            score += KNIGHT_ON_QUEEN * popcount(b & safe);
            let b = (self.att_by[us][BISHOP] & bishop_attacks(qs, self.occ))
                | (self.att_by[us][ROOK] & rook_attacks(qs, self.occ));
            score += SLIDER_ON_QUEEN * popcount(b & safe & self.att2[us]);
        }
        score
    }

    // ----------------------------------------------------------- Freibauern

    fn passers(&self, us: Color) -> S {
        let pos = self.pos;
        let them = us ^ 1;
        let mut score = S0;
        let mut b = self.passed[us];
        while b != 0 {
            let sq = pop_lsb(&mut b);
            let r = relative_rank(us, sq);
            let block = forward_sq(us, sq);
            let mut bonus = PASSED_RANK[r];

            if r >= 3 {
                let w = 5 * r as i32 - 13;
                // Koenigsnaehe (nur Endspiel)
                let mut kp = ((dist(self.ksq[them], block).min(5) * 19) / 4 - dist(self.ksq[us], block).min(5) * 2) * w;
                if r != 6 {
                    kp -= dist(self.ksq[us], forward_sq(us, block)).min(5) * w;
                }
                bonus.eg += kp * 45 / 100;

                if pos.board[block] == NO_PIECE {
                    let squares_to_queen = FORWARD_FILE[us][sq];
                    let mut unsafe_sq = PASSED_SPAN[us][sq];
                    let behind = FORWARD_FILE[them][sq] & (pos.by_type[ROOK] | pos.by_type[QUEEN]);
                    if pos.by_color[them] & behind == 0 {
                        unsafe_sq &= self.att[them] | pos.by_color[them];
                    }
                    let mut k = if unsafe_sq == 0 {
                        35
                    } else if unsafe_sq & squares_to_queen == 0 {
                        20
                    } else if unsafe_sq & sq_bb(block) == 0 {
                        9
                    } else {
                        0
                    };
                    if pos.by_color[us] & behind != 0 || self.att[us] & sq_bb(block) != 0 {
                        k += 5;
                    }
                    bonus += s(k * w * 6 / 10, k * w * 45 / 100);
                }
            }

            // Kandidaten und blockierte Doppelfreibauern nur halb
            if pos.pieces(them, PAWN) & PASSED_SPAN[us][block] != 0 || pos.by_type[PAWN] & sq_bb(block) != 0 {
                bonus = s(bonus.mg / 2, bonus.eg / 2);
            }
            let f = file_of(sq) as i32;
            score += bonus - PASSED_FILE * f.min(7 - f);
        }
        score
    }

    /// Quadratregel: Freibauer, den der gegnerische Koenig nicht mehr
    /// einholt, waehrend der Gegner keine Figuren mehr hat.
    fn unstoppable(&self) -> S {
        let pos = self.pos;
        let mut found = [false; 2];
        for us in [WHITE, BLACK] {
            let them = us ^ 1;
            if self.npm[them] != 0 {
                continue;
            }
            let mut b = pos.pieces(us, PAWN);
            while b != 0 {
                let sq = pop_lsb(&mut b);
                if pos.pieces(them, PAWN) & PASSED_SPAN[us][sq] != 0 || FORWARD_FILE[us][sq] & self.occ != 0 {
                    continue;
                }
                let r = relative_rank(us, sq) as i32;
                let pawn_moves = if r == 1 { 5 } else { 7 - r };
                let qsq = if us == WHITE { 56 + file_of(sq) } else { file_of(sq) };
                let mut king_moves = dist(self.ksq[them], qsq);
                if pos.side == them {
                    king_moves -= 1;
                }
                if king_moves > pawn_moves {
                    found[us] = true;
                    break;
                }
            }
        }
        match (found[WHITE], found[BLACK]) {
            (true, false) => UNSTOPPABLE_PASSER,
            (false, true) => -UNSTOPPABLE_PASSER,
            _ => S0,
        }
    }

    // ------------------------------------------------------- Raum/Initiative

    fn space(&self, us: Color) -> S {
        if self.npm[WHITE] + self.npm[BLACK] < SPACE_THRESHOLD {
            return S0;
        }
        let pos = self.pos;
        let them = us ^ 1;
        let safe = SPACE_MASK[us] & !pos.pieces(us, PAWN) & !self.pawn_att[them];
        let mut behind = pos.pieces(us, PAWN);
        behind |= shift_down(us, behind);
        behind |= shift_down(us, shift_down(us, behind));
        let bonus = popcount(safe) + popcount(behind & safe & !self.att[them]);
        let weight = popcount(pos.by_color[us]) - 1;
        s(bonus * weight * weight * 3 / 160, 0)
    }

    fn initiative(&self, score: S) -> S {
        let pos = self.pos;
        let wk = self.ksq[WHITE];
        let bk = self.ksq[BLACK];
        let outflanking =
            (file_of(wk) as i32 - file_of(bk) as i32).abs() - (rank_of(wk) as i32 - rank_of(bk) as i32).abs();
        let pawns = pos.by_type[PAWN];
        let both_flanks = pawns & QUEEN_SIDE != 0 && pawns & KING_SIDE != 0;
        let passed_count = popcount(self.passed[WHITE] | self.passed[BLACK]);
        let almost_unwinnable = passed_count == 0 && outflanking < 0 && !both_flanks;
        let infiltration = rank_of(wk) > 3 || rank_of(bk) < 4;
        let complexity = 9 * passed_count + 11 * popcount(pawns) + 9 * outflanking
            + 21 * both_flanks as i32
            + 24 * infiltration as i32
            + 51 * (self.npm[WHITE] + self.npm[BLACK] == 0) as i32
            - 43 * almost_unwinnable as i32
            - 110;
        let mg = score.mg;
        let eg = score.eg;
        let u = mg.signum() * ((complexity + 50) * 6 / 10).min(0).max(-mg.abs());
        let v = eg.signum() * (complexity * 45 / 100).max(-eg.abs());
        s(u, v)
    }

    // ------------------------------------------------------ Endspielfaktoren

    /// Skalierung (von 64) des Endspielwerts fuer die staerkere Seite.
    fn eg_scale(&self, strong: Color) -> i32 {
        let pos = self.pos;
        let weak = strong ^ 1;
        let sp = pos.pieces(strong, PAWN);
        let s_pieces = pos.by_color[strong] & !pos.by_type[PAWN] & !pos.by_type[KING];

        // Alle Bauern auf einer Randlinie: Festungen
        if sp != 0 && (sp & !FILE_A_BB == 0 || sp & !FILE_H_BB == 0) {
            let wk = self.ksq[weak];
            let qsq = if strong == WHITE { 56 + file_of(lsb(sp)) } else { file_of(lsb(sp)) };
            if s_pieces == 0 {
                // Koenig steht vor allen Randbauern
                if pos.by_color[weak] == pos.pieces(weak, KING) && sp & !PASSED_SPAN[weak][wk] == 0 {
                    return 0;
                }
            } else if s_pieces == pos.pieces(strong, BISHOP) && !more_than_one(s_pieces) {
                // Falscher Laeufer
                if is_dark(lsb(s_pieces)) != is_dark(qsq) && dist(qsq, wk) <= 1 {
                    return 0;
                }
            }
        }

        let wb = pos.pieces(WHITE, BISHOP);
        let bb = pos.pieces(BLACK, BISHOP);
        let ocb = wb != 0
            && bb != 0
            && !more_than_one(wb)
            && !more_than_one(bb)
            && (wb & DARK_SQUARES != 0) != (bb & DARK_SQUARES != 0);
        let sf = if ocb {
            if self.npm[WHITE] == MG_VALUE[BISHOP] && self.npm[BLACK] == MG_VALUE[BISHOP] {
                18 + 4 * popcount(self.passed[strong])
            } else {
                22 + 3 * popcount(pos.by_color[strong])
            }
        } else if self.npm[WHITE] == MG_VALUE[ROOK]
            && self.npm[BLACK] == MG_VALUE[ROOK]
            && popcount(sp) - popcount(pos.pieces(weak, PAWN)) <= 1
            && (sp & KING_SIDE != 0) != (sp & QUEEN_SIDE != 0)
            && king_attacks(self.ksq[weak]) & pos.pieces(weak, PAWN) != 0
        {
            36
        } else {
            36 + 7 * popcount(sp)
        };
        sf.min(64)
    }
}

// ---------------------------------------------------------------------------
// Spezielle Endspiele (gegen nackten Koenig)
// ---------------------------------------------------------------------------

/// Liefert die Bewertung (Sicht der Seite am Zug), wenn eine Seite nur noch
/// den Koenig hat und ein Spezialfall greift.
fn special_endgame(pos: &Position) -> Option<i32> {
    for strong in [WHITE, BLACK] {
        let weak = strong ^ 1;
        if pos.by_color[weak] != pos.pieces(weak, KING) {
            continue;
        }
        let sp = pos.pieces(strong, PAWN);
        let sn = pos.pieces(strong, KNIGHT);
        let sb = pos.pieces(strong, BISHOP);
        let heavy = pos.pieces(strong, ROOK) | pos.pieces(strong, QUEEN);
        let pieces = sn | sb | heavy;
        if pieces == 0 && sp == 0 {
            return Some(0);
        }
        let v = if sp == 0 {
            if heavy == 0 {
                if sb == 0 && popcount(sn) <= 2 {
                    return Some(0); // KNK, KNNK
                }
                if sn == 0 && (sb & DARK_SQUARES == 0 || sb & !DARK_SQUARES == 0) {
                    return Some(0); // nur gleichfarbige Laeufer
                }
                if popcount(sn) == 1 && popcount(sb) == 1 {
                    kbnk(pos, strong)
                } else {
                    kxk(pos, strong)
                }
            } else {
                kxk(pos, strong)
            }
        } else if pieces == 0 && !more_than_one(sp) {
            kpk(pos, strong)
        } else {
            let mut npm = 0;
            for pt in KNIGHT..KING {
                npm += MG_VALUE[pt] * popcount(pos.pieces(strong, pt));
            }
            if npm >= MG_VALUE[ROOK] {
                kxk(pos, strong)
            } else {
                return None;
            }
        };
        return Some(if pos.side == strong { v } else { -v });
    }
    None
}

/// Starke Seite gegen nackten Koenig: Material + Koenig an den Rand treiben.
fn kxk(pos: &Position, strong: Color) -> i32 {
    let weak = strong ^ 1;
    let wk = pos.king_sq(strong);
    let lk = pos.king_sq(weak);
    let mut material = 0;
    for pt in PAWN..KING {
        material += EG_VALUE[pt] * popcount(pos.pieces(strong, pt));
    }
    // Material deckeln, damit der Gradient auch bei absurder Uebermacht bleibt.
    let mut v = material.min(6000) + PUSH_CLOSE[DIST[wk][lk] as usize] + push_to_edge(lk);
    // Bauern vorruecken lassen
    let mut p = pos.pieces(strong, PAWN);
    while p != 0 {
        v += 10 * relative_rank(strong, pop_lsb(&mut p)) as i32;
    }
    let b = pos.pieces(strong, BISHOP);
    let n = pos.pieces(strong, KNIGHT);
    if pos.pieces(strong, QUEEN) != 0
        || pos.pieces(strong, ROOK) != 0
        || (b != 0 && n != 0)
        || (b & DARK_SQUARES != 0 && b & !DARK_SQUARES != 0)
        || popcount(n) >= 3
    {
        v += KNOWN_WIN;
    }
    v.min(EVAL_LIMIT - 1000)
}

/// KBNK: Koenig in die Ecke mit der Farbe des Laeufers treiben.
fn kbnk(pos: &Position, strong: Color) -> i32 {
    let weak = strong ^ 1;
    let wk = pos.king_sq(strong);
    let lk = pos.king_sq(weak);
    let bsq = lsb(pos.pieces(strong, BISHOP));
    let f = file_of(lk) as i32;
    let r = rank_of(lk) as i32;
    // Manhattan-Abstand zur naechsten Ecke in Laeuferfarbe (a1/h8 dunkel, h1/a8 hell)
    let d = if is_dark(bsq) { (f + r).min(14 - f - r) } else { (7 - f + r).min(f + 7 - r) };
    KNOWN_WIN + EG_VALUE[KNIGHT] + EG_VALUE[BISHOP] + (14 - d) * 20
        + push_to_edge(lk)
        + PUSH_CLOSE[DIST[wk][lk] as usize]
}

/// KPK ueber die Bitbasis.
fn kpk(pos: &Position, strong: Color) -> i32 {
    let weak = strong ^ 1;
    let mut wk = pos.king_sq(strong);
    let mut bk = pos.king_sq(weak);
    let mut p = lsb(pos.pieces(strong, PAWN));
    let mut stm = pos.side;
    if strong == BLACK {
        wk ^= 56;
        bk ^= 56;
        p ^= 56;
        stm ^= 1;
    }
    if file_of(p) >= 4 {
        wk ^= 7;
        bk ^= 7;
        p ^= 7;
    }
    if !kpk_probe(wk, p, bk, stm) {
        return 0;
    }
    KNOWN_WIN + EG_VALUE[PAWN] + 20 * rank_of(p) as i32
}

// ---------------------------------------------------------------------------
// KPK-Bitbasis (Retroanalyse, einmalig beim ersten Zugriff)
// ---------------------------------------------------------------------------

/// Seite am Zug * 24 Bauernfelder (Linien a-d, Reihen 2-7) * 64 * 64.
const KPK_SIZE: usize = 2 * 24 * 64 * 64;

static KPK: LazyLock<Vec<u64>> = LazyLock::new(kpk_generate);

/// Bit 0-5 weisser Koenig, 6-11 schwarzer Koenig, 12 Seite am Zug,
/// 13-14 Bauernlinie (a-d), 15-17 (Reihe 7 - Bauernreihe).
#[inline(always)]
fn kpk_index(stm: Color, bk: Square, wk: Square, psq: Square) -> usize {
    wk | (bk << 6) | (stm << 12) | (file_of(psq) << 13) | ((6 - rank_of(psq)) << 15)
}

/// Gewinnt Weiss (Koenig wk, Bauer psq auf Linie a-d) gegen Koenig bk?
fn kpk_probe(wk: Square, psq: Square, bk: Square, stm: Color) -> bool {
    debug_assert!(file_of(psq) <= 3 && (1..=6).contains(&rank_of(psq)));
    let idx = kpk_index(stm, bk, wk, psq);
    (KPK[idx >> 6] >> (idx & 63)) & 1 != 0
}

fn kpk_generate() -> Vec<u64> {
    const INVALID: u8 = 0;
    const UNKNOWN: u8 = 1;
    const DRAW: u8 = 2;
    const WIN: u8 = 4;

    #[derive(Clone, Copy)]
    struct Entry {
        stm: u8,
        wk: u8,
        bk: u8,
        psq: u8,
        res: u8,
    }

    let mut db: Vec<Entry> = Vec::with_capacity(KPK_SIZE);
    for idx in 0..KPK_SIZE {
        let wk = idx & 63;
        let bk = (idx >> 6) & 63;
        let stm = (idx >> 12) & 1;
        let psq = (6 - ((idx >> 15) & 7)) * 8 + ((idx >> 13) & 3);
        let res = if dist(wk, bk) <= 1
            || wk == psq
            || bk == psq
            || (stm == WHITE && pawn_attacks(WHITE, psq) & sq_bb(bk) != 0)
        {
            INVALID
        } else if stm == WHITE
            && rank_of(psq) == 6
            && wk != psq + 8
            && (dist(bk, psq + 8) > 1 || king_attacks(wk) & sq_bb(psq + 8) != 0)
        {
            WIN
        } else if stm == BLACK
            && (king_attacks(bk) & !(king_attacks(wk) | pawn_attacks(WHITE, psq)) == 0
                || king_attacks(bk) & sq_bb(psq) & !king_attacks(wk) != 0)
        {
            DRAW
        } else {
            UNKNOWN
        };
        db.push(Entry { stm: stm as u8, wk: wk as u8, bk: bk as u8, psq: psq as u8, res });
    }

    let classify = |db: &[Entry], e: Entry| -> u8 {
        let stm = e.stm as usize;
        let wk = e.wk as usize;
        let bk = e.bk as usize;
        let psq = e.psq as usize;
        let (good, bad) = if stm == WHITE { (WIN, DRAW) } else { (DRAW, WIN) };
        let mut r = INVALID;
        let mut b = king_attacks(if stm == WHITE { wk } else { bk });
        while b != 0 {
            let to = pop_lsb(&mut b);
            r |= if stm == WHITE {
                db[kpk_index(BLACK, bk, to, psq)].res
            } else {
                db[kpk_index(WHITE, to, wk, psq)].res
            };
        }
        if stm == WHITE {
            if rank_of(psq) < 6 {
                r |= db[kpk_index(BLACK, bk, wk, psq + 8)].res;
            }
            if rank_of(psq) == 1 && psq + 8 != wk && psq + 8 != bk {
                r |= db[kpk_index(BLACK, bk, wk, psq + 16)].res;
            }
        }
        if r & good != 0 {
            good
        } else if r & UNKNOWN != 0 {
            UNKNOWN
        } else {
            bad
        }
    };

    loop {
        let mut changed = false;
        for idx in 0..KPK_SIZE {
            if db[idx].res == UNKNOWN {
                let r = classify(&db, db[idx]);
                if r != UNKNOWN {
                    db[idx].res = r;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    let mut bits = vec![0u64; KPK_SIZE / 64];
    for (idx, e) in db.iter().enumerate() {
        if e.res == WIN {
            bits[idx >> 6] |= 1u64 << (idx & 63);
        }
    }
    bits
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::movegen::{generate, GenType, MoveList};
    use crate::position::START_FEN;

    /// xorshift64* mit festem Seed.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(2685821657736338717)
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    const PLAYOUT_FENS: [&str; 9] = [
        START_FEN,
        "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
        "r1bq1rk1/pp2bppp/2n1pn2/3p4/2PP4/2N1PN2/PP3PPP/R2QKB1R w KQ - 0 8",
        "2r2rk1/pp1bqppp/2n1pn2/3p4/3P4/2PBPN2/P1Q2PPP/R4RK1 b - - 3 14",
        "8/5pk1/6p1/3R4/7P/6P1/r4PK1/8 w - - 0 40",
        "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1",
        "4k3/8/3K4/3P4/8/8/8/8 w - - 0 1",
        "6k1/5ppp/8/8/8/8/1B3PPP/6K1 w - - 0 1",
        "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10",
    ];

    fn pos(fen: &str) -> Position {
        Position::from_fen(fen).unwrap_or_else(|e| panic!("{}: {}", fen, e))
    }

    /// Zufallspartien (Schlagzuege bevorzugt, damit auch Endspiele entstehen).
    fn playouts(games_per_fen: usize, max_plies: usize, seed: u64, mut f: impl FnMut(&Position)) {
        let mut rng = Rng(seed);
        for fen in PLAYOUT_FENS {
            for _ in 0..games_per_fen {
                let mut p = pos(fen);
                f(&p);
                for _ in 0..max_plies {
                    let mut list = MoveList::new();
                    generate(&p, &mut list, GenType::All);
                    let mut moves: Vec<Move> = list.as_slice().to_vec();
                    let mut played = false;
                    while !moves.is_empty() {
                        let caps: Vec<usize> = (0..moves.len()).filter(|&i| moves[i].is_capture()).collect();
                        let i = if !caps.is_empty() && rng.below(3) == 0 {
                            caps[rng.below(caps.len())]
                        } else {
                            rng.below(moves.len())
                        };
                        let m = moves.swap_remove(i);
                        if p.make_move(m) {
                            played = true;
                            break;
                        }
                    }
                    if !played || p.halfmove >= 100 {
                        break;
                    }
                    f(&p);
                }
            }
        }
    }

    fn check_symmetric(p: &Position) -> i32 {
        let v = evaluate(p);
        let m = p.mirrored();
        let vm = evaluate(&m);
        assert_eq!(v, vm, "Asymmetrie: {} -> {}, gespiegelt {} -> {}", p.to_fen(), v, m.to_fen(), vm);
        assert!(v.abs() <= EVAL_LIMIT);
        v
    }

    #[test]
    fn symmetry_random_playouts() {
        let mut n = 0usize;
        playouts(60, 200, 0xDEAD_BEEF_1234_5678, |p| {
            check_symmetric(p);
            n += 1;
        });
        println!("Symmetrie: {} Partiestellungen ok", n);
        assert!(n > 50_000);
    }

    /// FEN aus einem Brett mit zufaelligen Figuren.
    fn board_fen(board: &[u8; 64], side: Color) -> String {
        let mut s = String::new();
        for rank in (0..8).rev() {
            let mut empty = 0;
            for file in 0..8 {
                let p = board[rank * 8 + file];
                if p == NO_PIECE {
                    empty += 1;
                } else {
                    if empty > 0 {
                        s.push_str(&empty.to_string());
                        empty = 0;
                    }
                    s.push(piece_to_char(p));
                }
            }
            if empty > 0 {
                s.push_str(&empty.to_string());
            }
            if rank > 0 {
                s.push('/');
            }
        }
        s.push_str(if side == WHITE { " w - - 0 1" } else { " b - - 0 1" });
        s
    }

    #[test]
    fn symmetry_and_no_panic_random_placements() {
        let mut rng = Rng(0x0123_4567_89AB_CDEF);
        let mut n = 0usize;
        for _ in 0..200_000 {
            let mut board = [NO_PIECE; 64];
            let wk = rng.below(64);
            let mut bk = rng.below(64);
            while DIST[wk][bk] <= 1 {
                bk = rng.below(64);
            }
            board[wk] = make_piece(WHITE, KING);
            board[bk] = make_piece(BLACK, KING);
            let extra = rng.below(24);
            for _ in 0..extra {
                let c = rng.below(2);
                let pt = [PAWN, PAWN, PAWN, KNIGHT, BISHOP, ROOK, QUEEN][rng.below(7)];
                let sq = rng.below(64);
                if board[sq] != NO_PIECE || (pt == PAWN && (rank_of(sq) == 0 || rank_of(sq) == 7)) {
                    continue;
                }
                board[sq] = make_piece(c, pt);
            }
            let fen = board_fen(&board, rng.below(2));
            if let Ok(p) = Position::from_fen(&fen) {
                check_symmetric(&p);
                n += 1;
            }
        }
        println!("Symmetrie: {} Zufallsaufstellungen ok", n);
        assert!(n > 50_000);
    }

    #[test]
    fn no_panic_on_weird_positions() {
        let fens = [
            "QQQQQQQQ/Q7/8/8/8/8/7k/K7 b - - 0 1",
            "k7/8/8/8/8/8/q7/qqqqqqqK w - - 0 1",
            "K7/8/8/8/8/8/8/7k w - - 0 1",
            "7K/8/8/8/8/8/8/k7 b - - 0 1",
            "r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1",
            "NNNNNNNN/NN6/8/8/8/8/8/K5k1 b - - 0 1",
            "bbbbbbbb/bb6/8/8/8/8/8/K5k1 w - - 0 1",
            "RRRRRRRR/RR6/8/8/8/8/8/k1K5 b - - 0 1",
            "8/PPPPPPPP/8/8/8/8/pppppppp/K6k w - - 0 1",
            "k7/5PPP/8/8/8/8/5ppp/K7 w - - 0 1",
            "kQ6/8/1K6/8/8/8/8/8 b - - 0 1",
            "8/8/8/8/8/8/8/K1k5 w - - 0 1",
            "8/8/8/3k4/8/8/8/K7 w - - 99 120",
            "1k6/8/8/8/8/8/8/KBB5 w - - 0 1",
            "1k6/8/8/8/8/8/8/KB1B4 w - - 0 1",
            "1k6/8/8/8/8/8/8/KNNN4 w - - 0 1",
            "1k6/8/8/8/8/8/P7/KB6 w - - 0 1",
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 500 1",
            "7k/8/8/8/8/8/8/QQQQQQQK b - - 0 1",
            "4k3/8/8/8/8/8/8/4K2R w K - 0 1",
        ];
        for f in fens {
            let p = pos(f);
            let v = check_symmetric(&p);
            println!("{:6}  {}", v, f);
        }
    }

    #[test]
    fn sanity_startpos_and_material() {
        let v = evaluate(&pos(START_FEN));
        assert!(v.abs() <= 35, "Startstellung {}", v);
        let v = evaluate(&pos("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR b KQkq - 0 1"));
        assert!(v.abs() <= 35, "Startstellung (Schwarz am Zug) {}", v);

        // Eine Dame mehr
        let v = evaluate(&pos("rnb1kbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1"));
        assert!(v > 700, "Dame mehr (Weiss) {}", v);
        let v = evaluate(&pos("rnb1kbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR b KQkq - 0 1"));
        assert!(v < -700, "Dame mehr (Weiss), Schwarz am Zug {}", v);
        let v = evaluate(&pos("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNB1KBNR b KQkq - 0 1"));
        assert!(v > 700, "Dame mehr (Schwarz) {}", v);
        // Ein Turm, ein Springer mehr
        let v = evaluate(&pos("1nbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQk - 0 1"));
        assert!(v > 350, "Turm mehr {}", v);
        let v = evaluate(&pos("r1bqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1"));
        assert!(v > 220, "Springer mehr {}", v);
    }

    #[test]
    fn sanity_insufficient_material() {
        for f in [
            "8/8/8/4k3/8/8/8/3NK3 w - - 0 1",
            "8/8/8/4k3/8/8/8/3NK3 b - - 0 1",
            "8/8/8/4k3/8/8/8/3BK3 w - - 0 1",
            "8/8/8/4k3/8/8/8/3BK3 b - - 0 1",
            "8/8/8/4k3/8/8/8/2NNK3 w - - 0 1",
            "8/8/8/4k3/8/8/8/2NNK3 b - - 0 1",
            "3nk3/8/8/8/8/8/8/4K3 w - - 0 1",
            "2nnk3/8/8/8/8/8/8/4K3 b - - 0 1",
            "8/8/8/4k3/8/8/8/4K3 w - - 0 1",
            "8/8/4b3/4k3/8/8/8/3NK3 w - - 0 1",
            "8/8/4n3/4k3/8/8/8/3BK3 w - - 0 1",
            "8/8/4b3/4k3/8/8/8/2B1K3 w - - 0 1",
        ] {
            let v = evaluate(&pos(f));
            assert!(v.abs() <= 50, "{} -> {}", f, v);
        }
        // Turm gegen Leichtfigur: remisnah
        for f in ["8/8/4b3/4k3/8/8/8/3RK3 w - - 0 1", "8/8/4n3/4k3/8/8/8/3RK3 w - - 0 1"] {
            let v = evaluate(&pos(f));
            assert!(v.abs() <= 80, "{} -> {}", f, v);
        }
    }

    #[test]
    fn sanity_mating_material_and_mopup() {
        for f in [
            "8/8/8/4k3/8/8/8/3QK3 w - - 0 1",
            "8/8/8/4k3/8/8/8/3RK3 w - - 0 1",
            "8/8/8/4k3/8/8/8/1BB1K3 w - - 0 1",
            "8/8/8/4k3/8/8/8/1BN1K3 w - - 0 1",
        ] {
            let v = evaluate(&pos(f));
            assert!(v > 500, "{} -> {}", f, v);
            let fb = f.replace(" w ", " b ");
            let vb = evaluate(&pos(&fb));
            assert!(vb < -500, "{} -> {}", fb, vb);
        }
        // Schwarz stark
        let v = evaluate(&pos("4k3/8/8/8/3K4/8/8/7q b - - 0 1"));
        assert!(v > 500, "KQK Schwarz {}", v);

        // Gradient: Koenig am Rand/in der Ecke ist besser fuer die starke Seite
        // (gleicher Koenigsabstand 2 zum weissen Koenig auf c3)
        for (center, edge, corner) in [
            ("8/7Q/8/4k3/8/2K5/8/8 w - - 0 1", "8/7Q/8/k7/8/2K5/8/8 w - - 0 1", "8/7Q/8/8/8/2K5/8/k7 w - - 0 1"),
            ("8/7R/8/4k3/8/2K5/8/8 w - - 0 1", "8/7R/8/k7/8/2K5/8/8 w - - 0 1", "8/7R/8/8/8/2K5/8/k7 w - - 0 1"),
        ] {
            let (c, e, k) = (evaluate(&pos(center)), evaluate(&pos(edge)), evaluate(&pos(corner)));
            assert!(e > c && k > e, "Gradient {} {} {}", c, e, k);
        }
        // Naeher am verlierenden Koenig ist besser
        let far = evaluate(&pos("7k/8/8/8/8/8/8/K2Q4 w - - 0 1"));
        let near = evaluate(&pos("7k/8/5K2/8/8/8/8/3Q4 w - - 0 1"));
        assert!(near > far, "Koenigsnaehe {} {}", near, far);

        // KBNK: richtige Ecke (Laeuferfarbe) besser als falsche, Rand besser als Mitte
        let right = evaluate(&pos("7k/8/8/8/3K4/8/8/2BN4 w - - 0 1"));
        let wrong = evaluate(&pos("k7/8/8/8/3K4/8/8/2BN4 w - - 0 1"));
        let center = evaluate(&pos("8/8/8/4k3/8/8/1K6/2BN4 w - - 0 1"));
        assert!(right > wrong, "KBNK Ecke {} {}", right, wrong);
        assert!(right > center, "KBNK Mitte {} {}", right, center);
        assert!(wrong > 500 && center > 500);
    }

    #[test]
    fn sanity_kpk_bitbase() {
        let win = |f: &str| {
            let v = evaluate(&pos(f));
            assert!(v.abs() > 1000, "{} sollte gewonnen sein: {}", f, v);
            v
        };
        let draw = |f: &str| {
            let v = evaluate(&pos(f));
            assert_eq!(v, 0, "{} sollte remis sein", f);
        };
        assert!(win("4k3/8/4K3/4P3/8/8/8/8 w - - 0 1") > 0);
        assert!(win("4k3/8/4K3/4P3/8/8/8/8 b - - 0 1") < 0);
        draw("8/8/8/4k3/8/4K3/4P3/8 w - - 0 1");
        assert!(win("8/8/8/4k3/8/4K3/4P3/8 b - - 0 1") < 0);
        draw("k7/8/K7/P7/8/8/8/8 w - - 0 1");
        draw("7k/8/7K/7P/8/8/8/8 b - - 0 1");
        draw("k7/P7/1K6/8/8/8/8/8 b - - 0 1");
        assert!(win("8/4P3/8/8/8/k7/8/K7 w - - 0 1") > 0);
        // Schwarzer Bauer
        assert!(win("8/8/8/8/4p3/4k3/8/4K3 b - - 0 1") > 0);
        draw("8/4p3/4k3/8/4K3/8/8/8 b - - 0 1");
    }

    /// Jede KPK-Bewertung muss zur 1-Zug-Vorausschau (echte Zuggenerierung,
    /// beide Farben, alle Linien) passen. Prueft Index und Normierung.
    #[test]
    fn kpk_consistent_with_one_ply() {
        let mut rng = Rng(0x5EED_0F_C0FFEE);
        let mut checked = 0;
        let mut wins = 0;
        for _ in 0..60_000 {
            let strong = rng.below(2);
            let weak = strong ^ 1;
            let mut board = [NO_PIECE; 64];
            let psq = 8 + rng.below(48);
            let sk = rng.below(64);
            let wk = rng.below(64);
            if sk == psq || wk == psq || DIST[sk][wk] <= 1 {
                continue;
            }
            board[psq] = make_piece(strong, PAWN);
            board[sk] = make_piece(strong, KING);
            board[wk] = make_piece(weak, KING);
            let side = rng.below(2);
            let Ok(mut p) = Position::from_fen(&board_fen(&board, side)) else { continue };
            let v = evaluate(&p);
            assert!(v == 0 || v.abs() > 1000, "{} -> {}", p.to_fen(), v);
            let strong_wins = if side == strong { v > 1000 } else { v < -1000 };

            let moves = crate::movegen::legal_moves(&mut p);
            let expected = if side == strong {
                moves.iter().any(|&m| {
                    if m.is_promotion() {
                        m.promotion_type() == QUEEN
                            && (DIST[m.to()][wk] > 1 || DIST[m.to()][p.king_sq(strong)] <= 1)
                    } else {
                        p.make_move(m);
                        let w = -evaluate(&p) > 1000;
                        p.unmake_move();
                        w
                    }
                })
            } else {
                !moves.is_empty()
                    && moves.iter().all(|&m| {
                        p.make_move(m);
                        let w = p.pieces(strong, PAWN) != 0 && evaluate(&p) > 1000;
                        p.unmake_move();
                        w
                    })
            };
            assert_eq!(strong_wins, expected, "KPK inkonsistent: {} (Bewertung {})", p.to_fen(), v);
            checked += 1;
            wins += strong_wins as usize;
        }
        println!("KPK: {} Stellungen konsistent, davon {} gewonnen", checked, wins);
        assert!(checked > 20_000);
    }

    #[test]
    fn sanity_drawish_scaling() {
        // Ungleichfarbige Laeufer mit Mehrbauer: deutlich weniger als gleichfarbige
        let ocb = evaluate(&pos("4k3/8/4b3/8/3P4/8/2PB4/4K3 w - - 0 1"));
        let same = evaluate(&pos("4k3/4b3/8/8/3P4/8/2PB4/4K3 w - - 0 1"));
        assert!(ocb > 0 && ocb < same, "OCB {} gleichfarbig {}", ocb, same);
        // Falscher Laeufer mit Randbauer
        let v = evaluate(&pos("k7/8/8/8/8/8/P7/2B1K3 w - - 0 1"));
        assert!(v.abs() < 60, "falscher Laeufer {}", v);
        let v = evaluate(&pos("k7/8/8/8/8/8/P7/1B2K3 w - - 0 1"));
        assert!(v > 200, "richtiger Laeufer {}", v);
        // Randbauern vor dem Koenig
        let v = evaluate(&pos("k7/8/8/8/8/P7/P7/4K3 w - - 0 1"));
        assert!(v.abs() < 60, "Doppel-Randbauer {}", v);
    }

    #[test]
    fn sanity_unstoppable_passer() {
        let caught = evaluate(&pos("8/1k6/8/P5p1/8/8/6P1/K7 w - - 0 1"));
        let free = evaluate(&pos("8/8/8/P5p1/5k2/8/6P1/K7 w - - 0 1"));
        assert!(free > caught + 300, "Quadratregel {} {}", free, caught);
        assert!(free > 300, "unaufhaltsam {}", free);
    }

    #[test]
    fn sanity_passed_pawns_and_king_safety() {
        // Weiter vorgerueckter Freibauer ist mehr wert
        let a = evaluate(&pos("6k1/5ppp/8/3P4/8/8/5PPP/6K1 w - - 0 1"));
        let b = evaluate(&pos("6k1/5ppp/3P4/8/8/8/5PPP/6K1 w - - 0 1"));
        assert!(b > a, "Freibauer 6. Reihe {} vs 5. Reihe {}", b, a);
        // Intakter Bauernschild besser als aufgerissener
        let safe = evaluate(&pos("r1bq1rk1/pppp1ppp/2n2n2/4p3/4P3/2N2N2/PPPP1PPP/R1BQ1RK1 w - - 0 1"));
        let open = evaluate(&pos("r1bq1rk1/pppp1ppp/2n2n2/4p3/4P3/2N2N2/PPPP1P2/R1BQ1RK1 w - - 0 1"));
        assert!(safe > open, "Bauernschild {} {}", safe, open);
    }

    #[test]
    fn deterministic_across_threads() {
        let mut positions = Vec::new();
        playouts(4, 120, 77, |p| positions.push(p.clone()));
        let expected: Vec<i32> = positions.iter().map(evaluate).collect();
        std::thread::scope(|sc| {
            for _ in 0..4 {
                sc.spawn(|| {
                    for (p, &e) in positions.iter().zip(&expected) {
                        assert_eq!(evaluate(p), e);
                    }
                });
            }
        });
    }

    #[test]
    fn eval_speed() {
        init();
        let mut positions = Vec::new();
        let mut rng = Rng(99);
        playouts(40, 160, 4242, |p| {
            if positions.len() < 10_000 && rng.below(3) == 0 {
                positions.push(p.clone());
            }
        });
        let reps = 30;
        let t = std::time::Instant::now();
        let mut sum = 0i64;
        for _ in 0..reps {
            for p in &positions {
                sum += evaluate(std::hint::black_box(p)) as i64;
            }
        }
        let el = t.elapsed();
        let n = (reps * positions.len()) as f64;
        println!(
            "Geschwindigkeit: {} Stellungen x {}: {:.1} ns/Bewertung, {:.2} Mio. Bewertungen/s (Pruefsumme {})",
            positions.len(),
            reps,
            el.as_nanos() as f64 / n,
            n / el.as_secs_f64() / 1e6,
            sum
        );
    }
}
