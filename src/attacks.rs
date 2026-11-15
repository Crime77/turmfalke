//! Angriffstabellen: Bauer, Springer, Koenig und Magic Bitboards fuer
//! Laeufer/Turm. Die Magic-Zahlen werden beim Start mit festem Seed gesucht
//! (deterministisch, keine abgetippten Konstanten).

use crate::types::*;
use std::sync::LazyLock;

#[derive(Clone, Copy, Default)]
struct Magic {
    mask: Bitboard,
    magic: u64,
    shift: u32,
    offset: usize,
}

pub struct Tables {
    pawn: [[Bitboard; 64]; 2],
    knight: [Bitboard; 64],
    king: [Bitboard; 64],
    rook_magics: [Magic; 64],
    bishop_magics: [Magic; 64],
    slider_table: Vec<Bitboard>,
    between: Vec<Bitboard>, // 64 * 64
    line: Vec<Bitboard>,    // 64 * 64
}

static TABLES: LazyLock<Tables> = LazyLock::new(Tables::init);

/// Erzwingt den Aufbau der Tabellen (z. B. beim Programmstart).
pub fn init() {
    LazyLock::force(&TABLES);
}

const ROOK_DIRS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
const BISHOP_DIRS: [(i32, i32); 4] = [(1, 1), (1, -1), (-1, 1), (-1, -1)];

/// Langsame Referenzberechnung von Gleitangriffen (nur fuer die Initialisierung).
fn sliding_attack(sq: Square, occ: Bitboard, dirs: &[(i32, i32); 4]) -> Bitboard {
    let mut att = 0u64;
    for &(df, dr) in dirs {
        let mut f = file_of(sq) as i32 + df;
        let mut r = rank_of(sq) as i32 + dr;
        while (0..8).contains(&f) && (0..8).contains(&r) {
            let s = (r * 8 + f) as usize;
            att |= sq_bb(s);
            if occ & sq_bb(s) != 0 {
                break;
            }
            f += df;
            r += dr;
        }
    }
    att
}

struct Prng(u64);
impl Prng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(2685821657736338717)
    }
    fn sparse(&mut self) -> u64 {
        self.next() & self.next() & self.next()
    }
}

fn find_magic(sq: Square, dirs: &[(i32, i32); 4], table: &mut Vec<Bitboard>, rng: &mut Prng) -> Magic {
    let edges = ((RANK_1_BB | RANK_8_BB) & !rank_bb(rank_of(sq)))
        | ((FILE_A_BB | FILE_H_BB) & !file_bb(file_of(sq)));
    let mask = sliding_attack(sq, 0, dirs) & !edges;
    let bits = mask.count_ones();
    let size = 1usize << bits;
    let shift = 64 - bits;

    // Alle Teilmengen der Maske (Carry-Rippler) und ihre Referenzangriffe.
    let mut occs = Vec::with_capacity(size);
    let mut refs = Vec::with_capacity(size);
    let mut sub: u64 = 0;
    loop {
        occs.push(sub);
        refs.push(sliding_attack(sq, sub, dirs));
        sub = sub.wrapping_sub(mask) & mask;
        if sub == 0 {
            break;
        }
    }
    debug_assert_eq!(occs.len(), size);

    let offset = table.len();
    table.resize(offset + size, 0);
    let mut used = vec![0u64; size];
    let mut epoch = vec![0u32; size];
    let mut cur: u32 = 0;
    loop {
        let magic = rng.sparse();
        if (mask.wrapping_mul(magic) >> 56).count_ones() < 6 {
            continue;
        }
        cur += 1;
        let mut ok = true;
        for i in 0..size {
            let idx = (occs[i].wrapping_mul(magic) >> shift) as usize;
            if epoch[idx] != cur {
                epoch[idx] = cur;
                used[idx] = refs[i];
            } else if used[idx] != refs[i] {
                ok = false;
                break;
            }
        }
        if ok {
            table[offset..offset + size].copy_from_slice(&used);
            return Magic { mask, magic, shift, offset };
        }
    }
}

fn step_attacks(sq: Square, steps: &[(i32, i32)]) -> Bitboard {
    let mut att = 0u64;
    for &(df, dr) in steps {
        let f = file_of(sq) as i32 + df;
        let r = rank_of(sq) as i32 + dr;
        if (0..8).contains(&f) && (0..8).contains(&r) {
            att |= sq_bb((r * 8 + f) as usize);
        }
    }
    att
}

impl Tables {
    fn init() -> Tables {
        let mut t = Tables {
            pawn: [[0; 64]; 2],
            knight: [0; 64],
            king: [0; 64],
            rook_magics: [Magic::default(); 64],
            bishop_magics: [Magic::default(); 64],
            slider_table: Vec::with_capacity(110_000),
            between: vec![0; 64 * 64],
            line: vec![0; 64 * 64],
        };
        const KNIGHT_STEPS: [(i32, i32); 8] =
            [(1, 2), (2, 1), (2, -1), (1, -2), (-1, -2), (-2, -1), (-2, 1), (-1, 2)];
        const KING_STEPS: [(i32, i32); 8] =
            [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)];
        for sq in 0..64 {
            t.pawn[WHITE][sq] = step_attacks(sq, &[(-1, 1), (1, 1)]);
            t.pawn[BLACK][sq] = step_attacks(sq, &[(-1, -1), (1, -1)]);
            t.knight[sq] = step_attacks(sq, &KNIGHT_STEPS);
            t.king[sq] = step_attacks(sq, &KING_STEPS);
        }
        let mut rng = Prng(0x9E37_79B9_7F4A_7C15);
        for sq in 0..64 {
            t.rook_magics[sq] = find_magic(sq, &ROOK_DIRS, &mut t.slider_table, &mut rng);
        }
        for sq in 0..64 {
            t.bishop_magics[sq] = find_magic(sq, &BISHOP_DIRS, &mut t.slider_table, &mut rng);
        }
        for a in 0..64 {
            for b in 0..64 {
                if a == b {
                    continue;
                }
                let ra = sliding_attack(a, 0, &ROOK_DIRS);
                let ba = sliding_attack(a, 0, &BISHOP_DIRS);
                if ra & sq_bb(b) != 0 {
                    let rb = sliding_attack(b, 0, &ROOK_DIRS);
                    t.line[a * 64 + b] = (ra & rb) | sq_bb(a) | sq_bb(b);
                    t.between[a * 64 + b] =
                        sliding_attack(a, sq_bb(b), &ROOK_DIRS) & sliding_attack(b, sq_bb(a), &ROOK_DIRS);
                } else if ba & sq_bb(b) != 0 {
                    let bb = sliding_attack(b, 0, &BISHOP_DIRS);
                    t.line[a * 64 + b] = (ba & bb) | sq_bb(a) | sq_bb(b);
                    t.between[a * 64 + b] = sliding_attack(a, sq_bb(b), &BISHOP_DIRS)
                        & sliding_attack(b, sq_bb(a), &BISHOP_DIRS);
                }
            }
        }
        t
    }
}

/// Felder, die ein Bauer der Farbe c auf sq angreift.
#[inline(always)]
pub fn pawn_attacks(c: Color, sq: Square) -> Bitboard {
    TABLES.pawn[c][sq]
}
#[inline(always)]
pub fn knight_attacks(sq: Square) -> Bitboard {
    TABLES.knight[sq]
}
#[inline(always)]
pub fn king_attacks(sq: Square) -> Bitboard {
    TABLES.king[sq]
}
#[inline(always)]
pub fn bishop_attacks(sq: Square, occ: Bitboard) -> Bitboard {
    let t = &*TABLES;
    let m = &t.bishop_magics[sq];
    t.slider_table[m.offset + ((occ & m.mask).wrapping_mul(m.magic) >> m.shift) as usize]
}
#[inline(always)]
pub fn rook_attacks(sq: Square, occ: Bitboard) -> Bitboard {
    let t = &*TABLES;
    let m = &t.rook_magics[sq];
    t.slider_table[m.offset + ((occ & m.mask).wrapping_mul(m.magic) >> m.shift) as usize]
}
#[inline(always)]
pub fn queen_attacks(sq: Square, occ: Bitboard) -> Bitboard {
    bishop_attacks(sq, occ) | rook_attacks(sq, occ)
}
/// Felder strikt zwischen a und b (0, wenn nicht auf einer Linie).
#[inline(always)]
pub fn between(a: Square, b: Square) -> Bitboard {
    TABLES.between[a * 64 + b]
}
/// Ganze Linie (Rand zu Rand) durch a und b (0, wenn nicht auf einer Linie).
#[inline(always)]
pub fn line(a: Square, b: Square) -> Bitboard {
    TABLES.line[a * 64 + b]
}
/// Angriffe einer Figur vom Typ pt auf sq (Bauern ausgenommen).
#[inline(always)]
pub fn piece_attacks(pt: usize, sq: Square, occ: Bitboard) -> Bitboard {
    match pt {
        KNIGHT => knight_attacks(sq),
        BISHOP => bishop_attacks(sq, occ),
        ROOK => rook_attacks(sq, occ),
        QUEEN => queen_attacks(sq, occ),
        KING => king_attacks(sq),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magics_match_reference() {
        // Pseudozufaellige Belegungen gegen die langsame Referenz pruefen.
        let mut rng = Prng(12345);
        for sq in 0..64 {
            for _ in 0..2000 {
                let occ = rng.next() & rng.next();
                assert_eq!(rook_attacks(sq, occ), sliding_attack(sq, occ, &ROOK_DIRS));
                assert_eq!(bishop_attacks(sq, occ), sliding_attack(sq, occ, &BISHOP_DIRS));
            }
        }
    }

    #[test]
    fn simple_tables() {
        assert_eq!(knight_attacks(0), sq_bb(10) | sq_bb(17));
        assert_eq!(king_attacks(0), sq_bb(1) | sq_bb(8) | sq_bb(9));
        assert_eq!(pawn_attacks(WHITE, 8), sq_bb(17));
        assert_eq!(pawn_attacks(BLACK, 49), sq_bb(40) | sq_bb(42));
        assert_eq!(between(0, 63).count_ones(), 6);
        assert_eq!(between(0, 7).count_ones(), 6);
        assert_eq!(between(0, 10), 0);
        assert_eq!(line(0, 9).count_ones(), 8);
    }
}
