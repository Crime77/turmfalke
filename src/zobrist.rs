//! Zobrist-Schluessel fuer das Hashing von Stellungen (fester Seed).

use std::sync::LazyLock;

pub struct Zobrist {
    pub psq: [[u64; 64]; 12],
    pub castling: [u64; 16],
    pub ep: [u64; 8],
    pub side: u64,
}

pub static ZOBRIST: LazyLock<Zobrist> = LazyLock::new(|| {
    // SplitMix64 mit festem Seed.
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = move || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    let mut z = Zobrist { psq: [[0; 64]; 12], castling: [0; 16], ep: [0; 8], side: 0 };
    for p in 0..12 {
        for sq in 0..64 {
            z.psq[p][sq] = next();
        }
    }
    for i in 0..16 {
        z.castling[i] = next();
    }
    for i in 0..8 {
        z.ep[i] = next();
    }
    z.side = next();
    z
});
