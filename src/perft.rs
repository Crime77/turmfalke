//! Perft: zaehlt alle Zugfolgen bis Tiefe N (Pruefung des Zuggenerators).

use crate::movegen::{generate, GenType, MoveList};
use crate::position::Position;
use crate::types::Move;

pub fn perft(pos: &mut Position, depth: u32) -> u64 {
    if depth == 0 {
        return 1;
    }
    let mut list = MoveList::new();
    generate(pos, &mut list, GenType::All);
    let mut nodes = 0;
    for &m in list.as_slice() {
        if pos.make_move(m) {
            nodes += if depth == 1 { 1 } else { perft(pos, depth - 1) };
            pos.unmake_move();
        }
    }
    nodes
}

/// Perft mit Aufschluesselung nach Wurzelzuegen.
pub fn divide(pos: &mut Position, depth: u32) -> (Vec<(Move, u64)>, u64) {
    let mut out = Vec::new();
    let mut total = 0;
    if depth == 0 {
        return (out, 1);
    }
    let mut list = MoveList::new();
    generate(pos, &mut list, GenType::All);
    for &m in list.as_slice() {
        if pos.make_move(m) {
            let n = perft(pos, depth - 1);
            pos.unmake_move();
            out.push((m, n));
            total += n;
        }
    }
    (out, total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::position::START_FEN;

    pub const KIWIPETE: &str = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";
    pub const POS3: &str = "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1";
    pub const POS4: &str = "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1";
    pub const POS4_MIRROR: &str = "r2q1rk1/pP1p2pp/Q4n2/bbp1p3/Np6/1B3NBn/pPPP1PPP/R3K2R b KQ - 0 1";
    pub const POS5: &str = "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8";
    pub const POS6: &str = "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10";

    fn check(fen: &str, expected: &[u64]) {
        let mut pos = Position::from_fen(fen).unwrap();
        for (i, &e) in expected.iter().enumerate() {
            let d = i as u32 + 1;
            let n = perft(&mut pos, d);
            assert_eq!(n, e, "perft({}) falsch fuer {}", d, fen);
        }
        assert_eq!(pos.to_fen().split_whitespace().take(4).collect::<Vec<_>>(),
                   fen.split_whitespace().take(4).collect::<Vec<_>>());
    }

    #[test]
    fn perft_startpos() {
        check(START_FEN, &[20, 400, 8_902, 197_281, 4_865_609, 119_060_324]);
    }
    #[test]
    fn perft_kiwipete() {
        check(KIWIPETE, &[48, 2_039, 97_862, 4_085_603, 193_690_690]);
    }
    #[test]
    fn perft_pos3() {
        check(POS3, &[14, 191, 2_812, 43_238, 674_624, 11_030_083]);
    }
    #[test]
    fn perft_pos4() {
        check(POS4, &[6, 264, 9_467, 422_333, 15_833_292]);
        check(POS4_MIRROR, &[6, 264, 9_467, 422_333, 15_833_292]);
    }
    #[test]
    fn perft_pos5() {
        check(POS5, &[44, 1_486, 62_379, 2_103_487, 89_941_194]);
    }
    #[test]
    fn perft_pos6() {
        check(POS6, &[46, 2_079, 89_890, 3_894_594]);
    }

    /// Nach jedem Zug muss der inkrementelle Hash stimmen und unmake muss
    /// den Zustand exakt wiederherstellen.
    fn walk(pos: &mut Position, depth: u32) {
        if depth == 0 {
            return;
        }
        let mut list = MoveList::new();
        generate(pos, &mut list, GenType::All);
        for &m in list.as_slice() {
            let before = pos.snapshot();
            if pos.make_move(m) {
                assert_eq!(pos.hash, pos.compute_hash(), "Hash falsch nach {}", m.to_uci());
                let fen = pos.to_fen();
                let re = Position::from_fen(&fen).unwrap();
                assert_eq!(re.hash, pos.hash, "FEN-Rundreise aendert Hash: {}", fen);
                walk(pos, depth - 1);
                pos.unmake_move();
            }
            assert_eq!(pos.snapshot(), before, "unmake stellt Zustand nicht her ({})", m.to_uci());
        }
        // Nullzug
        if !pos.in_check() {
            let before = pos.snapshot();
            pos.make_null_move();
            assert_eq!(pos.hash, pos.compute_hash());
            pos.unmake_null_move();
            assert_eq!(pos.snapshot(), before);
        }
    }

    #[test]
    fn hash_and_unmake_consistency() {
        for fen in [START_FEN, KIWIPETE, POS3, POS4, POS5, POS6] {
            let mut pos = Position::from_fen(fen).unwrap();
            walk(&mut pos, 3);
        }
    }

    #[test]
    fn mirror_perft_symmetry() {
        for fen in [START_FEN, KIWIPETE, POS3, POS4, POS5, POS6] {
            let mut a = Position::from_fen(fen).unwrap();
            let mut b = a.mirrored();
            assert_eq!(perft(&mut a, 3), perft(&mut b, 3), "Spiegel-Perft weicht ab: {}", fen);
        }
    }

    #[test]
    fn invalid_fens_rejected() {
        for fen in [
            "",
            "8/8/8/8/8/8/8/8 w - - 0 1",
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR x KQkq - 0 1",
            "rnbqkbnr/pppppppp/9/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP w KQkq - 0 1",
            "4k3/8/8/8/8/8/8/4K2P w - - 0 1",
            "4k3/4Q3/8/8/8/8/8/4K3 w - - 0 1", // Schwarz im Schach, Weiss am Zug
        ] {
            assert!(Position::from_fen(fen).is_err(), "sollte ungueltig sein: {}", fen);
        }
    }

    #[test]
    fn repetition_and_draws() {
        let mut pos = Position::startpos();
        for mv in ["g1f3", "g8f6", "f3g1", "f6g8", "g1f3", "g8f6", "f3g1", "f6g8"] {
            let m = pos.parse_uci_move(mv).unwrap();
            assert!(pos.make_move(m));
        }
        // Startstellung zum dritten Mal -> Remis (auch an der Wurzel, ply = 0)
        assert!(pos.is_repetition(0));
        let kk = Position::from_fen("8/8/4k3/8/8/3K4/8/8 w - - 0 1").unwrap();
        assert!(kk.is_insufficient_material());
        let kbk = Position::from_fen("8/8/4k3/8/8/3KB3/8/8 w - - 0 1").unwrap();
        assert!(kbk.is_insufficient_material());
        let krk = Position::from_fen("8/8/4k3/8/8/3K4/8/R7 w - - 0 1").unwrap();
        assert!(!krk.is_insufficient_material());
    }

    #[test]
    fn see_basics() {
        // Bauer schlaegt ungedeckten Springer: gut.
        let mut p = Position::from_fen("4k3/8/8/3n4/4P3/8/8/4K3 w - - 0 1").unwrap();
        let m = p.parse_uci_move("e4d5").unwrap();
        assert!(p.see_ge(m, 0));
        assert!(p.see_ge(m, 300));
        assert!(!p.see_ge(m, 301));
        // Dame schlaegt durch Bauer gedeckten Bauern: schlecht.
        let mut p = Position::from_fen("4k3/8/2p5/3p4/8/8/8/3QK3 w - - 0 1").unwrap();
        let m = p.parse_uci_move("d1d5").unwrap();
        assert!(!p.see_ge(m, 0));
        assert!(p.see_ge(m, -800));
    }
}
