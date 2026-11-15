//! Brettdarstellung: FEN, Zug ausfuehren/zuruecknehmen, Hashing, Angriffe,
//! Statische Abtauschbewertung (SEE) und Remis-Erkennung.

use crate::attacks::*;
use crate::movegen;
use crate::types::*;
use crate::zobrist::ZOBRIST;

pub const START_FEN: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

/// Rochaderechte, die erhalten bleiben, wenn ein Zug dieses Feld beruehrt.
const CASTLE_MASK: [u8; 64] = {
    let mut m = [15u8; 64];
    m[A1] = 15 & !CASTLE_WQ;
    m[E1] = 15 & !(CASTLE_WK | CASTLE_WQ);
    m[H1] = 15 & !CASTLE_WK;
    m[A8] = 15 & !CASTLE_BQ;
    m[E8] = 15 & !(CASTLE_BK | CASTLE_BQ);
    m[H8] = 15 & !CASTLE_BK;
    m
};

/// Figurenwerte fuer die SEE (Koenig = 0, wie ueblich).
pub const SEE_VALUES: [i32; 6] = [100, 300, 300, 500, 900, 0];

#[inline(always)]
fn see_value(p: u8) -> i32 {
    if p == NO_PIECE {
        0
    } else {
        SEE_VALUES[piece_type(p)]
    }
}

#[derive(Clone, Copy)]
struct StateInfo {
    mv: Move,
    captured: u8,
    castling: u8,
    ep: u8,
    halfmove: u32,
    plies_from_null: u32,
    hash: u64,
}

#[derive(Clone)]
pub struct Position {
    pub by_type: [Bitboard; 6],
    pub by_color: [Bitboard; 2],
    pub board: [u8; 64],
    pub side: Color,
    pub castling: u8,
    /// en-passant-Feld; nur gesetzt, wenn ein Bauer dort wirklich schlagen kann.
    pub ep: Square,
    pub halfmove: u32,
    pub fullmove: u32,
    pub hash: u64,
    plies_from_null: u32,
    history: Vec<StateInfo>,
}

impl Position {
    fn empty() -> Position {
        Position {
            by_type: [0; 6],
            by_color: [0; 2],
            board: [NO_PIECE; 64],
            side: WHITE,
            castling: 0,
            ep: NO_SQ,
            halfmove: 0,
            fullmove: 1,
            hash: 0,
            plies_from_null: 0,
            history: Vec::with_capacity(512),
        }
    }

    pub fn startpos() -> Position {
        Position::from_fen(START_FEN).expect("Startstellung ist gueltig")
    }

    // ------------------------------------------------------------------
    // Abfragen
    // ------------------------------------------------------------------

    #[inline(always)]
    pub fn pieces(&self, c: Color, pt: usize) -> Bitboard {
        self.by_type[pt] & self.by_color[c]
    }
    #[inline(always)]
    pub fn occupied(&self) -> Bitboard {
        self.by_color[WHITE] | self.by_color[BLACK]
    }
    #[inline(always)]
    pub fn king_sq(&self, c: Color) -> Square {
        lsb(self.pieces(c, KING))
    }
    #[inline(always)]
    pub fn piece_on(&self, sq: Square) -> u8 {
        self.board[sq]
    }
    /// Hat Farbe c andere Figuren als Bauern und Koenig?
    #[inline(always)]
    pub fn has_non_pawn_material(&self, c: Color) -> bool {
        self.by_color[c] & !(self.by_type[PAWN] | self.by_type[KING]) != 0
    }
    /// Anzahl der Halbzuege seit Beginn der Zugfolge (inkl. "position ... moves").
    #[inline(always)]
    pub fn game_ply(&self) -> usize {
        self.history.len()
    }
    /// Der zuletzt ausgefuehrte Zug (oder Move::NONE).
    #[inline(always)]
    pub fn last_move(&self) -> Move {
        match self.history.last() {
            Some(st) => st.mv,
            None => Move::NONE,
        }
    }
    /// Die geschlagene Figur des zuletzt ausgefuehrten Zuges (oder NO_PIECE).
    #[inline(always)]
    pub fn last_captured(&self) -> u8 {
        match self.history.last() {
            Some(st) => st.captured,
            None => NO_PIECE,
        }
    }
    /// Figur, die durch Zug m geschlagen wuerde (NO_PIECE, wenn keine).
    #[inline(always)]
    pub fn captured_piece(&self, m: Move) -> u8 {
        if m.is_ep() {
            make_piece(self.side ^ 1, PAWN)
        } else if m.is_capture() {
            self.board[m.to()]
        } else {
            NO_PIECE
        }
    }

    #[inline(always)]
    fn put_piece(&mut self, sq: Square, p: u8) {
        let b = sq_bb(sq);
        self.board[sq] = p;
        self.by_type[piece_type(p)] |= b;
        self.by_color[piece_color(p)] |= b;
    }
    #[inline(always)]
    fn remove_piece(&mut self, sq: Square) {
        let p = self.board[sq];
        let b = sq_bb(sq);
        self.board[sq] = NO_PIECE;
        self.by_type[piece_type(p)] &= !b;
        self.by_color[piece_color(p)] &= !b;
    }
    #[inline(always)]
    fn move_piece(&mut self, from: Square, to: Square) {
        let p = self.board[from];
        let b = sq_bb(from) | sq_bb(to);
        self.board[from] = NO_PIECE;
        self.board[to] = p;
        self.by_type[piece_type(p)] ^= b;
        self.by_color[piece_color(p)] ^= b;
    }

    /// Hash komplett neu berechnen (fuer FEN und Selbsttests).
    pub fn compute_hash(&self) -> u64 {
        let z = &*ZOBRIST;
        let mut h = 0u64;
        for sq in 0..64 {
            let p = self.board[sq];
            if p != NO_PIECE {
                h ^= z.psq[p as usize][sq];
            }
        }
        h ^= z.castling[self.castling as usize];
        if self.ep != NO_SQ {
            h ^= z.ep[file_of(self.ep)];
        }
        if self.side == BLACK {
            h ^= z.side;
        }
        h
    }

    /// Wird Feld sq von Farbe `by` angegriffen?
    #[inline]
    pub fn is_attacked(&self, sq: Square, by: Color) -> bool {
        let occ = self.occupied();
        let them = self.by_color[by];
        (pawn_attacks(by ^ 1, sq) & self.by_type[PAWN] & them) != 0
            || (knight_attacks(sq) & self.by_type[KNIGHT] & them) != 0
            || (king_attacks(sq) & self.by_type[KING] & them) != 0
            || (bishop_attacks(sq, occ) & (self.by_type[BISHOP] | self.by_type[QUEEN]) & them) != 0
            || (rook_attacks(sq, occ) & (self.by_type[ROOK] | self.by_type[QUEEN]) & them) != 0
    }

    /// Alle Figuren (beider Farben), die sq bei Belegung occ angreifen.
    #[inline]
    pub fn attackers_to(&self, sq: Square, occ: Bitboard) -> Bitboard {
        (pawn_attacks(BLACK, sq) & self.pieces(WHITE, PAWN))
            | (pawn_attacks(WHITE, sq) & self.pieces(BLACK, PAWN))
            | (knight_attacks(sq) & self.by_type[KNIGHT])
            | (king_attacks(sq) & self.by_type[KING])
            | (bishop_attacks(sq, occ) & (self.by_type[BISHOP] | self.by_type[QUEEN]))
            | (rook_attacks(sq, occ) & (self.by_type[ROOK] | self.by_type[QUEEN]))
    }

    /// Steht die Seite am Zug im Schach?
    #[inline]
    pub fn in_check(&self) -> bool {
        self.is_attacked(self.king_sq(self.side), self.side ^ 1)
    }

    // ------------------------------------------------------------------
    // Zuege
    // ------------------------------------------------------------------

    /// Fuehrt einen pseudo-legalen Zug aus. Laesst er den eigenen Koenig im
    /// Schach, wird er sofort zurueckgenommen und `false` geliefert.
    pub fn make_move(&mut self, m: Move) -> bool {
        let z = &*ZOBRIST;
        let us = self.side;
        let them = us ^ 1;
        let from = m.from();
        let to = m.to();
        let flags = m.flags();
        let piece = self.board[from];
        debug_assert!(piece != NO_PIECE && piece_color(piece) == us);
        let pt = piece_type(piece);

        self.history.push(StateInfo {
            mv: m,
            captured: NO_PIECE,
            castling: self.castling,
            ep: self.ep as u8,
            halfmove: self.halfmove,
            plies_from_null: self.plies_from_null,
            hash: self.hash,
        });

        let mut h = self.hash;
        if self.ep != NO_SQ {
            h ^= z.ep[file_of(self.ep)];
            self.ep = NO_SQ;
        }
        self.halfmove += 1;
        self.plies_from_null += 1;

        let mut captured = NO_PIECE;
        if flags == EP_CAPTURE {
            let cap_sq = if us == WHITE { to - 8 } else { to + 8 };
            captured = self.board[cap_sq];
            debug_assert!(captured == make_piece(them, PAWN));
            self.remove_piece(cap_sq);
            h ^= z.psq[captured as usize][cap_sq];
            self.halfmove = 0;
        } else if m.is_capture() {
            captured = self.board[to];
            debug_assert!(captured != NO_PIECE && piece_color(captured) == them);
            self.remove_piece(to);
            h ^= z.psq[captured as usize][to];
            self.halfmove = 0;
        }

        self.move_piece(from, to);
        h ^= z.psq[piece as usize][from] ^ z.psq[piece as usize][to];

        if pt == PAWN {
            self.halfmove = 0;
            if m.is_promotion() {
                let promo = make_piece(us, m.promotion_type());
                self.remove_piece(to);
                self.put_piece(to, promo);
                h ^= z.psq[piece as usize][to] ^ z.psq[promo as usize][to];
            } else if flags == DOUBLE_PUSH {
                let ep_sq = if us == WHITE { from + 8 } else { from - 8 };
                if pawn_attacks(us, ep_sq) & self.pieces(them, PAWN) != 0 {
                    self.ep = ep_sq;
                    h ^= z.ep[file_of(ep_sq)];
                }
            }
        } else if flags == KING_CASTLE || flags == QUEEN_CASTLE {
            let (rf, rt) = if flags == KING_CASTLE { (from + 3, from + 1) } else { (from - 4, from - 1) };
            let rook = self.board[rf];
            debug_assert!(rook == make_piece(us, ROOK));
            self.move_piece(rf, rt);
            h ^= z.psq[rook as usize][rf] ^ z.psq[rook as usize][rt];
        }

        let new_castling = self.castling & CASTLE_MASK[from] & CASTLE_MASK[to];
        if new_castling != self.castling {
            h ^= z.castling[self.castling as usize] ^ z.castling[new_castling as usize];
            self.castling = new_castling;
        }

        self.side = them;
        h ^= z.side;
        if us == BLACK {
            self.fullmove += 1;
        }
        self.hash = h;
        if let Some(st) = self.history.last_mut() {
            st.captured = captured;
        }

        if self.is_attacked(self.king_sq(us), them) {
            self.unmake_move();
            return false;
        }
        // en-passant-Feld nur, wenn das Schlagen wirklich legal ist (sonst
        // haetten FIDE-gleiche Stellungen verschiedene Hashes).
        if self.ep != NO_SQ && !self.ep_capture_legal() {
            self.hash ^= z.ep[file_of(self.ep)];
            self.ep = NO_SQ;
        }
        true
    }

    /// Kann die Seite am Zug auf dem gesetzten en-passant-Feld legal schlagen?
    fn ep_capture_legal(&mut self) -> bool {
        let us = self.side;
        let ep = self.ep;
        let mut att = pawn_attacks(us ^ 1, ep) & self.pieces(us, PAWN);
        while att != 0 {
            let from = pop_lsb(&mut att);
            if self.make_move(Move::new(from, ep, EP_CAPTURE)) {
                self.unmake_move();
                return true;
            }
        }
        false
    }

    /// Nimmt den zuletzt mit make_move ausgefuehrten Zug zurueck.
    pub fn unmake_move(&mut self) {
        let st = self.history.pop().expect("unmake_move ohne Zug");
        let m = st.mv;
        let us = self.side ^ 1; // Seite, die gezogen hatte
        self.side = us;
        if us == BLACK {
            self.fullmove -= 1;
        }
        let from = m.from();
        let to = m.to();
        let flags = m.flags();

        if m.is_promotion() {
            self.remove_piece(to);
            self.put_piece(to, make_piece(us, PAWN));
        }
        self.move_piece(to, from);
        if flags == KING_CASTLE || flags == QUEEN_CASTLE {
            let (rf, rt) = if flags == KING_CASTLE { (from + 3, from + 1) } else { (from - 4, from - 1) };
            self.move_piece(rt, rf);
        }
        if st.captured != NO_PIECE {
            let cap_sq = if flags == EP_CAPTURE {
                if us == WHITE {
                    to - 8
                } else {
                    to + 8
                }
            } else {
                to
            };
            self.put_piece(cap_sq, st.captured);
        }
        self.castling = st.castling;
        self.ep = st.ep as Square;
        self.halfmove = st.halfmove;
        self.plies_from_null = st.plies_from_null;
        self.hash = st.hash;
    }

    /// Nullzug (Seite am Zug passt) fuer Null-Move-Pruning.
    pub fn make_null_move(&mut self) {
        let z = &*ZOBRIST;
        self.history.push(StateInfo {
            mv: Move::NULL,
            captured: NO_PIECE,
            castling: self.castling,
            ep: self.ep as u8,
            halfmove: self.halfmove,
            plies_from_null: self.plies_from_null,
            hash: self.hash,
        });
        let mut h = self.hash;
        if self.ep != NO_SQ {
            h ^= z.ep[file_of(self.ep)];
            self.ep = NO_SQ;
        }
        h ^= z.side;
        self.side ^= 1;
        self.halfmove += 1;
        self.plies_from_null = 0;
        self.hash = h;
    }

    pub fn unmake_null_move(&mut self) {
        let st = self.history.pop().expect("unmake_null_move ohne Zug");
        debug_assert!(st.mv == Move::NULL);
        self.side ^= 1;
        self.castling = st.castling;
        self.ep = st.ep as Square;
        self.halfmove = st.halfmove;
        self.plies_from_null = st.plies_from_null;
        self.hash = st.hash;
    }

    // ------------------------------------------------------------------
    // Remis-Erkennung
    // ------------------------------------------------------------------

    /// Wiederholung: innerhalb des Suchbaums genuegt eine Wiederholung,
    /// davor (Partiegeschichte) braucht es eine dreifache.
    pub fn is_repetition(&self, ply: usize) -> bool {
        let n = self.history.len();
        let limit = (self.halfmove.min(self.plies_from_null) as usize).min(n);
        let mut count = 0;
        let mut i = 4;
        while i <= limit {
            if self.history[n - i].hash == self.hash {
                if i < ply {
                    return true;
                }
                count += 1;
                if count >= 2 {
                    return true;
                }
            }
            i += 2;
        }
        false
    }

    /// Kein Matt mehr moeglich (K-K, K+Leichtfigur-K, nur gleichfarbige Laeufer).
    pub fn is_insufficient_material(&self) -> bool {
        if self.by_type[PAWN] | self.by_type[ROOK] | self.by_type[QUEEN] != 0 {
            return false;
        }
        let minors = self.by_type[KNIGHT] | self.by_type[BISHOP];
        if !more_than_one(minors) {
            return true;
        }
        if self.by_type[KNIGHT] == 0 {
            let b = self.by_type[BISHOP];
            if b & DARK_SQUARES == 0 || b & !DARK_SQUARES == 0 {
                return true;
            }
        }
        false
    }

    /// Remis durch 50-Zuege-Regel, Wiederholung oder ungenuegendes Material.
    pub fn is_draw(&mut self, ply: usize) -> bool {
        if self.halfmove >= 100 && (!self.in_check() || movegen::has_legal_move(self)) {
            return true;
        }
        self.is_insufficient_material() || self.is_repetition(ply)
    }

    // ------------------------------------------------------------------
    // Statische Abtauschbewertung
    // ------------------------------------------------------------------

    /// Ist der Materialsaldo des Zuges m nach allen Abtauschen >= threshold?
    pub fn see_ge(&self, m: Move, threshold: i32) -> bool {
        if m.is_castle() || m.is_ep() || m.is_promotion() {
            return 0 >= threshold;
        }
        let from = m.from();
        let to = m.to();
        let mut swap = see_value(self.board[to]) - threshold;
        if swap < 0 {
            return false;
        }
        swap = see_value(self.board[from]) - swap;
        if swap <= 0 {
            return true;
        }
        let mut occ = self.occupied() ^ sq_bb(from) ^ sq_bb(to);
        let mut stm = self.side;
        let mut attackers = self.attackers_to(to, occ);
        let bishops_queens = self.by_type[BISHOP] | self.by_type[QUEEN];
        let rooks_queens = self.by_type[ROOK] | self.by_type[QUEEN];
        let mut res: i32 = 1;
        loop {
            stm ^= 1;
            attackers &= occ;
            let stm_attackers = attackers & self.by_color[stm];
            if stm_attackers == 0 {
                break;
            }
            res ^= 1;

            // Schwaechsten Angreifer waehlen (Bauer, Springer, Laeufer, Turm, Dame).
            let mut pt = PAWN;
            while pt < KING && stm_attackers & self.by_type[pt] == 0 {
                pt += 1;
            }
            if pt == KING {
                // Nur der Koenig kann schlagen: das darf er nur, wenn der
                // Gegner das Feld nicht mehr angreift.
                return if attackers & !self.by_color[stm] != 0 { res ^ 1 != 0 } else { res != 0 };
            }
            swap = SEE_VALUES[pt] - swap;
            if swap < res {
                break;
            }
            occ ^= sq_bb(lsb(stm_attackers & self.by_type[pt]));
            // Durch das Wegnehmen werden evtl. Gleitfiguren dahinter frei (X-Ray).
            if pt == PAWN || pt == BISHOP || pt == QUEEN {
                attackers |= bishop_attacks(to, occ) & bishops_queens;
            }
            if pt == ROOK || pt == QUEEN {
                attackers |= rook_attacks(to, occ) & rooks_queens;
            }
        }
        res != 0
    }

    // ------------------------------------------------------------------
    // FEN / Ein- und Ausgabe
    // ------------------------------------------------------------------

    pub fn from_fen(fen: &str) -> Result<Position, String> {
        let parts: Vec<&str> = fen.split_whitespace().collect();
        if parts.len() < 4 {
            return Err("FEN braucht mindestens 4 Felder".into());
        }
        let mut pos = Position::empty();

        let rows: Vec<&str> = parts[0].split('/').collect();
        if rows.len() != 8 {
            return Err("FEN: es muessen 8 Reihen sein".into());
        }
        for (i, row) in rows.iter().enumerate() {
            let rank = 7 - i;
            let mut file = 0usize;
            for ch in row.chars() {
                if let Some(d) = ch.to_digit(10) {
                    if d == 0 || d > 8 {
                        return Err("FEN: ungueltige Ziffer".into());
                    }
                    file += d as usize;
                } else {
                    let p = piece_from_char(ch).ok_or_else(|| format!("FEN: unbekannte Figur '{}'", ch))?;
                    if file >= 8 {
                        return Err("FEN: Reihe zu lang".into());
                    }
                    pos.put_piece(rank * 8 + file, p);
                    file += 1;
                }
                if file > 8 {
                    return Err("FEN: Reihe zu lang".into());
                }
            }
            if file != 8 {
                return Err("FEN: Reihe hat nicht 8 Felder".into());
            }
        }

        pos.side = match parts[1] {
            "w" => WHITE,
            "b" => BLACK,
            _ => return Err("FEN: Seite am Zug muss w oder b sein".into()),
        };

        if parts[2] != "-" {
            for ch in parts[2].chars() {
                // Neben KQkq auch X-FEN/Shredder-Schreibweise (Turmlinie: H/A bzw. h/a).
                pos.castling |= match ch {
                    'K' | 'H' => CASTLE_WK,
                    'Q' | 'A' => CASTLE_WQ,
                    'k' | 'h' => CASTLE_BK,
                    'q' | 'a' => CASTLE_BQ,
                    'B'..='G' | 'b'..='g' => 0, // Chess960-Rochaden werden nicht unterstuetzt
                    _ => return Err("FEN: ungueltige Rochaderechte".into()),
                };
            }
        }

        // Plausibilitaet: genau ein Koenig je Seite, keine Bauern auf Reihe 1/8,
        // hoechstens 16 Steine je Seite (haelt die Zugliste begrenzt).
        for c in [WHITE, BLACK] {
            if popcount(pos.pieces(c, KING)) != 1 {
                return Err("FEN: jede Seite braucht genau einen Koenig".into());
            }
            if popcount(pos.by_color[c]) > 16 {
                return Err("FEN: mehr als 16 Steine fuer eine Seite".into());
            }
        }
        if pos.by_type[PAWN] & (RANK_1_BB | RANK_8_BB) != 0 {
            return Err("FEN: Bauern auf der 1. oder 8. Reihe".into());
        }

        // Rochaderechte nur behalten, wenn Koenig und Turm auf den Ausgangsfeldern stehen.
        let wk = make_piece(WHITE, KING);
        let wr = make_piece(WHITE, ROOK);
        let bk = make_piece(BLACK, KING);
        let br = make_piece(BLACK, ROOK);
        if pos.board[E1] != wk || pos.board[H1] != wr {
            pos.castling &= !CASTLE_WK;
        }
        if pos.board[E1] != wk || pos.board[A1] != wr {
            pos.castling &= !CASTLE_WQ;
        }
        if pos.board[E8] != bk || pos.board[H8] != br {
            pos.castling &= !CASTLE_BK;
        }
        if pos.board[E8] != bk || pos.board[A8] != br {
            pos.castling &= !CASTLE_BQ;
        }

        // en passant: nur uebernehmen, wenn es zur Stellung passt und schlagbar ist.
        if parts[3] != "-" {
            let sq = parse_square(parts[3]).ok_or("FEN: ungueltiges en-passant-Feld")?;
            let us = pos.side;
            let them = us ^ 1;
            let (ep_rank, pawn_sq, origin_sq) =
                if us == WHITE { (5, sq.wrapping_sub(8), sq + 8) } else { (2, sq + 8, sq.wrapping_sub(8)) };
            if rank_of(sq) == ep_rank
                && pos.board[sq] == NO_PIECE
                && pos.board[origin_sq] == NO_PIECE
                && pos.board[pawn_sq] == make_piece(them, PAWN)
                && pawn_attacks(them, sq) & pos.pieces(us, PAWN) != 0
            {
                pos.ep = sq;
            }
        }

        pos.halfmove = parts.get(4).and_then(|s| s.parse::<u32>().ok()).unwrap_or(0).min(10_000);
        pos.fullmove = parts.get(5).and_then(|s| s.parse::<u32>().ok()).unwrap_or(1).clamp(1, 100_000);

        // Die Seite, die nicht am Zug ist, darf nicht im Schach stehen.
        if pos.is_attacked(pos.king_sq(pos.side ^ 1), pos.side) {
            return Err("FEN: der Gegner steht im Schach (illegale Stellung)".into());
        }
        pos.hash = pos.compute_hash();
        // en passant nur behalten, wenn das Schlagen auch legal ist.
        if pos.ep != NO_SQ && !pos.ep_capture_legal() {
            pos.ep = NO_SQ;
            pos.hash = pos.compute_hash();
        }
        Ok(pos)
    }

    pub fn to_fen(&self) -> String {
        let mut s = String::new();
        for rank in (0..8).rev() {
            let mut empty = 0;
            for file in 0..8 {
                let p = self.board[rank * 8 + file];
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
        s.push(' ');
        s.push(if self.side == WHITE { 'w' } else { 'b' });
        s.push(' ');
        if self.castling == 0 {
            s.push('-');
        } else {
            if self.castling & CASTLE_WK != 0 {
                s.push('K');
            }
            if self.castling & CASTLE_WQ != 0 {
                s.push('Q');
            }
            if self.castling & CASTLE_BK != 0 {
                s.push('k');
            }
            if self.castling & CASTLE_BQ != 0 {
                s.push('q');
            }
        }
        s.push(' ');
        if self.ep == NO_SQ {
            s.push('-');
        } else {
            s.push_str(&square_name(self.ep));
        }
        s.push_str(&format!(" {} {}", self.halfmove, self.fullmove));
        s
    }

    /// Brett als Text (fuer den Befehl `d`).
    pub fn pretty(&self) -> String {
        let mut s = String::from("\n +---+---+---+---+---+---+---+---+\n");
        for rank in (0..8).rev() {
            for file in 0..8 {
                let p = self.board[rank * 8 + file];
                let c = if p == NO_PIECE { ' ' } else { piece_to_char(p) };
                s.push_str(&format!(" | {}", c));
            }
            s.push_str(&format!(" | {}\n +---+---+---+---+---+---+---+---+\n", rank + 1));
        }
        s.push_str("   a   b   c   d   e   f   g   h\n\n");
        s.push_str(&format!("Fen: {}\nKey: {:016X}\n", self.to_fen(), self.hash));
        s
    }

    /// Wandelt einen UCI-Zug (z. B. "e2e4", "e7e8q") in einen legalen Zug um.
    pub fn parse_uci_move(&mut self, s: &str) -> Option<Move> {
        let s = s.trim().to_ascii_lowercase();
        movegen::legal_moves(self).into_iter().find(|m| m.to_uci() == s)
    }

    /// Farbgespiegelte Stellung (fuer Symmetrietests der Bewertung).
    pub fn mirrored(&self) -> Position {
        let mut p = Position::empty();
        for sq in 0..64 {
            let pc = self.board[sq];
            if pc != NO_PIECE {
                p.put_piece(sq ^ 56, make_piece(piece_color(pc) ^ 1, piece_type(pc)));
            }
        }
        p.side = self.side ^ 1;
        p.castling = ((self.castling & 3) << 2) | ((self.castling >> 2) & 3);
        p.ep = if self.ep == NO_SQ { NO_SQ } else { self.ep ^ 56 };
        p.halfmove = self.halfmove;
        p.fullmove = self.fullmove;
        p.hash = p.compute_hash();
        p
    }

    /// Vergleichsschluessel des kompletten Zustands (fuer Selbsttests).
    #[cfg(test)]
    pub fn snapshot(&self) -> (Vec<u64>, Vec<u8>, usize, u8, usize, u32, u32, u64, usize) {
        let mut bbs = self.by_type.to_vec();
        bbs.extend_from_slice(&self.by_color);
        (
            bbs,
            self.board.to_vec(),
            self.side,
            self.castling,
            self.ep,
            self.halfmove,
            self.fullmove,
            self.hash,
            self.history.len(),
        )
    }
}
