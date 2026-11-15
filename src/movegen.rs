//! Zuggenerator: pseudo-legale Zuege (Legalitaet prueft make_move).

use crate::attacks::*;
use crate::position::Position;
use crate::types::*;

/// Obergrenze fuer pseudo-legale Zuege: max. 16 Steine je Seite x 27 Zuege = 432.
pub const MAX_MOVES: usize = 512;

pub struct MoveList {
    pub moves: [Move; MAX_MOVES],
    pub len: usize,
}

impl MoveList {
    #[inline]
    pub fn new() -> MoveList {
        MoveList { moves: [Move::NONE; MAX_MOVES], len: 0 }
    }
    #[inline(always)]
    fn push(&mut self, m: Move) {
        if self.len < MAX_MOVES {
            self.moves[self.len] = m;
            self.len += 1;
        }
    }
    #[inline(always)]
    pub fn as_slice(&self) -> &[Move] {
        &self.moves[..self.len]
    }
    #[inline(always)]
    pub fn len(&self) -> usize {
        self.len
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GenType {
    /// Alle pseudo-legalen Zuege.
    All,
    /// Nur Schlagzuege (inkl. en passant) und Damen-Umwandlungen.
    Noisy,
}

#[inline(always)]
fn push_promotions(list: &mut MoveList, from: Square, to: Square, base: u16, gt: GenType) {
    list.push(Move::new(from, to, base | 3)); // Dame
    if gt == GenType::All {
        list.push(Move::new(from, to, base)); // Springer
        list.push(Move::new(from, to, base | 2)); // Turm
        list.push(Move::new(from, to, base | 1)); // Laeufer
    }
}

fn gen_pawns(pos: &Position, list: &mut MoveList, gt: GenType) {
    let us = pos.side;
    let them = us ^ 1;
    let enemy = pos.by_color[them];
    let empty = !pos.occupied();
    let pawns = pos.pieces(us, PAWN);
    let (promo_rank, start_rank) = if us == WHITE { (rank_bb(6), rank_bb(1)) } else { (rank_bb(1), rank_bb(6)) };

    let mut bb = pawns;
    while bb != 0 {
        let from = pop_lsb(&mut bb);
        let to = if us == WHITE { from + 8 } else { from - 8 };
        let mut caps = pawn_attacks(us, from) & enemy;
        if sq_bb(from) & promo_rank != 0 {
            if empty & sq_bb(to) != 0 {
                push_promotions(list, from, to, PROMO, gt);
            }
            while caps != 0 {
                let t = pop_lsb(&mut caps);
                push_promotions(list, from, t, PROMO_CAPTURE, gt);
            }
        } else {
            if gt == GenType::All && empty & sq_bb(to) != 0 {
                list.push(Move::new(from, to, QUIET));
                if sq_bb(from) & start_rank != 0 {
                    let to2 = if us == WHITE { to + 8 } else { to - 8 };
                    if empty & sq_bb(to2) != 0 {
                        list.push(Move::new(from, to2, DOUBLE_PUSH));
                    }
                }
            }
            while caps != 0 {
                let t = pop_lsb(&mut caps);
                list.push(Move::new(from, t, CAPTURE));
            }
        }
    }

    if pos.ep != NO_SQ {
        let mut att = pawn_attacks(them, pos.ep) & pawns;
        while att != 0 {
            let from = pop_lsb(&mut att);
            list.push(Move::new(from, pos.ep, EP_CAPTURE));
        }
    }
}

fn gen_castling(pos: &Position, list: &mut MoveList) {
    let us = pos.side;
    let them = us ^ 1;
    let occ = pos.occupied();
    let (k_from, rights_k, rights_q, h_rook, a_rook) = if us == WHITE {
        (E1, CASTLE_WK, CASTLE_WQ, H1, A1)
    } else {
        (E8, CASTLE_BK, CASTLE_BQ, H8, A8)
    };
    if pos.castling & (rights_k | rights_q) == 0 {
        return;
    }
    let king = make_piece(us, KING);
    let rook = make_piece(us, ROOK);
    if pos.board[k_from] != king || pos.is_attacked(k_from, them) {
        return;
    }
    // kurze Rochade: f- und g-Feld frei und nicht angegriffen
    if pos.castling & rights_k != 0
        && pos.board[h_rook] == rook
        && occ & (sq_bb(k_from + 1) | sq_bb(k_from + 2)) == 0
        && !pos.is_attacked(k_from + 1, them)
        && !pos.is_attacked(k_from + 2, them)
    {
        list.push(Move::new(k_from, k_from + 2, KING_CASTLE));
    }
    // lange Rochade: b-, c-, d-Feld frei; c- und d-Feld nicht angegriffen
    if pos.castling & rights_q != 0
        && pos.board[a_rook] == rook
        && occ & (sq_bb(k_from - 1) | sq_bb(k_from - 2) | sq_bb(k_from - 3)) == 0
        && !pos.is_attacked(k_from - 1, them)
        && !pos.is_attacked(k_from - 2, them)
    {
        list.push(Move::new(k_from, k_from - 2, QUEEN_CASTLE));
    }
}

/// Erzeugt pseudo-legale Zuege fuer die Seite am Zug.
pub fn generate(pos: &Position, list: &mut MoveList, gt: GenType) {
    let us = pos.side;
    let own = pos.by_color[us];
    let enemy = pos.by_color[us ^ 1];
    let occ = own | enemy;
    let targets = if gt == GenType::All { !own } else { enemy };

    gen_pawns(pos, list, gt);

    for pt in [KNIGHT, BISHOP, ROOK, QUEEN, KING] {
        let mut bb = pos.pieces(us, pt);
        while bb != 0 {
            let from = pop_lsb(&mut bb);
            let mut att = piece_attacks(pt, from, occ) & targets;
            while att != 0 {
                let to = pop_lsb(&mut att);
                let flag = if enemy & sq_bb(to) != 0 { CAPTURE } else { QUIET };
                list.push(Move::new(from, to, flag));
            }
        }
    }

    if gt == GenType::All {
        gen_castling(pos, list);
    }
}

/// Alle legalen Zuege (langsam; fuer Wurzel und UCI-Eingabe).
pub fn legal_moves(pos: &mut Position) -> Vec<Move> {
    let mut list = MoveList::new();
    generate(pos, &mut list, GenType::All);
    let mut out = Vec::with_capacity(list.len());
    for &m in list.as_slice() {
        if pos.make_move(m) {
            pos.unmake_move();
            out.push(m);
        }
    }
    out
}

/// Gibt es mindestens einen legalen Zug?
pub fn has_legal_move(pos: &mut Position) -> bool {
    let mut list = MoveList::new();
    generate(pos, &mut list, GenType::All);
    for &m in list.as_slice() {
        if pos.make_move(m) {
            pos.unmake_move();
            return true;
        }
    }
    false
}
