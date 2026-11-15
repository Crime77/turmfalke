//! Suche: iterative Vertiefung, Aspiration Windows, PVS, Quiescence,
//! Null-Move, Reverse Futility, Razoring, Futility, LMR, LMP, SEE-Pruning,
//! Schachverlaengerung, Mate-Distance-Pruning.
//! Zugsortierung: TT-Zug, gute Schlagzuege, Killer, Countermove, History.

use crate::eval::evaluate;
use crate::movegen::{generate, GenType, MoveList, MAX_MOVES};
use crate::position::{Position, SEE_VALUES};
use crate::time::{now_ms, Limits, TimePlan};
use crate::tt::{BOUND_EXACT, BOUND_LOWER, BOUND_UPPER, TT};
use crate::types::*;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed, Ordering::SeqCst};
use std::sync::{Arc, LazyLock};

pub const MAX_PLY: usize = 128;
pub const INF: i32 = 32_000;
pub const MATE: i32 = 31_000;
/// Alles oberhalb ist ein Mattwert.
pub const MATE_IN_MAX: i32 = MATE - 2 * MAX_PLY as i32;
/// Keine statische Bewertung (im Schach).
pub const EVAL_NONE: i32 = -32_001;
pub const MAX_THREADS: usize = 256;

/// Gibt eine Zeile auf stdout aus (zeilenweise gesperrt und sofort geflusht).
pub fn send(line: &str) {
    let out = std::io::stdout();
    let mut lock = out.lock();
    let _ = writeln!(lock, "{}", line);
    let _ = lock.flush();
}

/// Von allen Such-Threads geteilter Zustand.
pub struct Shared {
    pub tt: TT,
    /// Alle Such-Threads sollen aufhoeren zu suchen (Zeit, Knoten, stop).
    pub stop: AtomicBool,
    /// Der UCI-Befehl `stop` (oder quit) ist eingetroffen: bei `go infinite`
    /// bzw. `go ponder` darf jetzt `bestmove` gesendet werden.
    pub uci_stop: AtomicBool,
    pub pondering: AtomicBool,
    /// Startzeit der Zeitmessung (ms); wird bei `ponderhit` neu gesetzt.
    pub start_ms: AtomicU64,
    /// Nummer der aktuellen Suche; aeltere Suchen brechen ab.
    pub generation: AtomicU64,
    pub nodes: Vec<AtomicU64>,
}

impl Shared {
    pub fn new(hash_mb: usize) -> Shared {
        Shared {
            tt: TT::new(hash_mb),
            stop: AtomicBool::new(false),
            uci_stop: AtomicBool::new(false),
            pondering: AtomicBool::new(false),
            start_ms: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            nodes: (0..MAX_THREADS).map(|_| AtomicU64::new(0)).collect(),
        }
    }
    pub fn total_nodes(&self) -> u64 {
        self.nodes.iter().map(|n| n.load(Relaxed)).sum()
    }
}

/// Lerntabellen eines Threads, die zwischen Zuegen erhalten bleiben.
pub struct ThreadData {
    pub history: [[[i32; 64]; 64]; 2],
    pub countermoves: [[Move; 64]; 12],
}

impl ThreadData {
    pub fn new() -> Box<ThreadData> {
        Box::new(ThreadData { history: [[[0; 64]; 64]; 2], countermoves: [[Move::NONE; 64]; 12] })
    }
}

#[derive(Clone, Copy)]
struct Frame {
    static_eval: i32,
    mv: Move,
    piece: u8,
}

static LMR: LazyLock<[[i32; 64]; 64]> = LazyLock::new(|| {
    let mut t = [[0i32; 64]; 64];
    for (d, row) in t.iter_mut().enumerate().skip(1) {
        for (m, v) in row.iter_mut().enumerate().skip(1) {
            *v = (0.75 + (d as f64).ln() * (m as f64).ln() / 2.25) as i32;
        }
    }
    t
});

#[inline(always)]
fn score_to_tt(s: i32, ply: usize) -> i32 {
    if s >= MATE_IN_MAX {
        s + ply as i32
    } else if s <= -MATE_IN_MAX {
        s - ply as i32
    } else {
        s
    }
}

#[inline(always)]
fn score_from_tt(s: i32, ply: usize) -> i32 {
    if s >= MATE_IN_MAX {
        s - ply as i32
    } else if s <= -MATE_IN_MAX {
        s + ply as i32
    } else {
        s
    }
}

pub fn score_string(score: i32) -> String {
    if score >= MATE_IN_MAX {
        format!("mate {}", (MATE - score + 1) / 2)
    } else if score <= -MATE_IN_MAX {
        format!("mate {}", -((MATE + score) / 2))
    } else {
        format!("cp {}", score)
    }
}

// Werte fuer die Zugsortierung.
const SCORE_TT: i32 = 1_000_000_000;
const SCORE_GOOD_CAPTURE: i32 = 400_000_000;
const SCORE_QUEEN_PROMO: i32 = 390_000_000;
const SCORE_KILLER1: i32 = 300_000_000;
const SCORE_KILLER2: i32 = 290_000_000;
const SCORE_COUNTER: i32 = 280_000_000;
const SCORE_BAD_CAPTURE: i32 = -400_000_000;
const SCORE_UNDERPROMO: i32 = -500_000_000;
const MVV: [i32; 6] = [100, 300, 310, 500, 900, 0];

pub struct Worker {
    pub id: usize,
    generation: u64,
    restrict_root: bool,
    pub pos: Position,
    pub td: Box<ThreadData>,
    shared: Arc<Shared>,
    limits: Limits,
    plan: TimePlan,
    go_ms: u64,
    nodes: u64,
    seldepth: usize,
    stopped: bool,
    root_depth: i32,
    completed_depth: i32,
    killers: [[Move; 2]; MAX_PLY + 4],
    stack: [Frame; MAX_PLY + 4],
    pv: Box<[[Move; MAX_PLY + 1]; MAX_PLY + 1]>,
    pv_len: [usize; MAX_PLY + 1],
    root_moves: Vec<Move>,
    pub best_move: Move,
    pub best_score: i32,
    pub best_pv: Vec<Move>,
}

impl Worker {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: usize,
        generation: u64,
        pos: Position,
        td: Box<ThreadData>,
        shared: Arc<Shared>,
        limits: Limits,
        plan: TimePlan,
        root_moves: Vec<Move>,
        restrict_root: bool,
    ) -> Worker {
        Worker {
            id,
            generation,
            restrict_root,
            pos,
            td,
            go_ms: shared.start_ms.load(Relaxed),
            shared,
            limits,
            plan,
            nodes: 0,
            seldepth: 0,
            stopped: false,
            root_depth: 0,
            completed_depth: 0,
            killers: [[Move::NONE; 2]; MAX_PLY + 4],
            stack: [Frame { static_eval: EVAL_NONE, mv: Move::NONE, piece: NO_PIECE }; MAX_PLY + 4],
            pv: Box::new([[Move::NONE; MAX_PLY + 1]; MAX_PLY + 1]),
            pv_len: [0; MAX_PLY + 1],
            root_moves,
            best_move: Move::NONE,
            best_score: 0,
            best_pv: Vec::new(),
        }
    }

    pub fn nodes(&self) -> u64 {
        self.nodes
    }

    /// Bester Zug nach der Suche; garantiert ein legaler Wurzelzug.
    pub fn final_best_move(&self) -> Move {
        if !self.best_move.is_none() && self.root_moves.contains(&self.best_move) {
            self.best_move
        } else {
            self.root_moves.first().copied().unwrap_or(Move::NONE)
        }
    }

    /// Zweiter Zug der Hauptvariante (fuer `bestmove ... ponder ...`).
    pub fn ponder_move(&self) -> Option<Move> {
        if self.best_pv.len() >= 2 && self.best_pv[0] == self.final_best_move() {
            Some(self.best_pv[1])
        } else {
            None
        }
    }

    #[inline(always)]
    fn check_stop(&mut self) -> bool {
        if self.stopped {
            return true;
        }
        if self.nodes & 1023 == 0 {
            // Eine neuere Suche wurde gestartet: diese ist veraltet.
            if self.shared.generation.load(Relaxed) != self.generation {
                self.stopped = true;
                return true;
            }
            self.shared.nodes[self.id].store(self.nodes, Relaxed);
            if self.shared.stop.load(Relaxed) {
                self.stopped = true;
                return true;
            }
            if self.id == 0 && self.completed_depth >= 1 {
                if let Some(n) = self.limits.nodes {
                    if self.shared.total_nodes() >= n {
                        self.shared.stop.store(true, Relaxed);
                        self.stopped = true;
                    }
                }
                if self.plan.timed && !self.shared.pondering.load(SeqCst) {
                    let elapsed = now_ms().saturating_sub(self.shared.start_ms.load(SeqCst));
                    if elapsed >= self.plan.hard {
                        self.shared.stop.store(true, Relaxed);
                        self.stopped = true;
                    }
                }
            }
        }
        self.stopped
    }

    // ------------------------------------------------------------------
    // Iterative Vertiefung
    // ------------------------------------------------------------------

    pub fn iterate(&mut self) {
        let main = self.id == 0;
        let max_depth = self.limits.depth.unwrap_or(MAX_PLY as i32 - 1).clamp(1, MAX_PLY as i32 - 1);
        let mut score: i32 = 0;
        let mut prev_best = Move::NONE;
        let mut stability = 0;

        for depth in 1..=max_depth {
            self.root_depth = depth;
            self.seldepth = 0;
            let mut delta = 18;
            let (mut alpha, mut beta) = (-INF, INF);
            if depth >= 4 && score.abs() < MATE_IN_MAX {
                alpha = (score - delta).max(-INF);
                beta = (score + delta).min(INF);
            }
            let mut this_score;
            loop {
                this_score = self.negamax(alpha, beta, depth, 0, false);
                // Ein Wurzelzug, der alpha verbessert hat, wurde vollstaendig
                // durchsucht - auch bei Abbruch ist er verwertbar.
                if self.pv_len[0] > 0 {
                    self.best_move = self.pv[0][0];
                    self.best_pv = self.pv[0][..self.pv_len[0]].to_vec();
                }
                if self.stopped {
                    break;
                }
                if this_score <= alpha {
                    beta = (alpha + beta) / 2;
                    alpha = (this_score - delta).max(-INF);
                } else if this_score >= beta {
                    beta = (this_score + delta).min(INF);
                } else {
                    break;
                }
                delta += delta / 2;
                if delta > 1000 {
                    alpha = -INF;
                    beta = INF;
                }
            }
            if self.stopped {
                break;
            }
            score = this_score;
            self.best_score = score;
            self.completed_depth = depth;
            if main {
                self.shared.nodes[0].store(self.nodes, Relaxed);
                if !self.limits.silent {
                    self.report(depth, score);
                }
                if self.best_move != prev_best {
                    stability = 0;
                } else {
                    stability += 1;
                }
                prev_best = self.best_move;

                if let Some(n) = self.limits.mate {
                    if score >= MATE_IN_MAX && (MATE - score + 1) / 2 <= n {
                        break;
                    }
                }
                if self.plan.timed && !self.shared.pondering.load(SeqCst) {
                    // Nur ein legaler Zug: sofort spielen.
                    if self.root_moves.len() == 1 {
                        break;
                    }
                    // Matt sicher gefunden: nicht weiter Zeit verbrauchen.
                    if score.abs() >= MATE_IN_MAX && depth > (MATE - score.abs()) + 6 {
                        break;
                    }
                    // Weiche Grenze (bei fester Zugzeit `movetime` gilt nur die harte).
                    if !self.plan.fixed {
                        let elapsed = now_ms().saturating_sub(self.shared.start_ms.load(SeqCst));
                        let pct: u64 = match stability {
                            0 => 130,
                            1 => 110,
                            2 | 3 => 100,
                            _ => 75,
                        };
                        if elapsed >= self.plan.soft.saturating_mul(pct) / 100 {
                            break;
                        }
                    }
                }
            }
        }

        if main {
            // Bei `go infinite` / `go ponder` erst nach stop bzw. ponderhit antworten.
            while !self.shared.uci_stop.load(SeqCst)
                && self.shared.generation.load(SeqCst) == self.generation
                && (self.limits.infinite || self.shared.pondering.load(SeqCst))
            {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        if self.shared.generation.load(Relaxed) == self.generation {
            self.shared.nodes[self.id].store(self.nodes, Relaxed);
        }
    }

    fn report(&self, depth: i32, score: i32) {
        let elapsed = now_ms().saturating_sub(self.go_ms).max(1);
        let nodes = self.shared.total_nodes();
        let nps = nodes * 1000 / elapsed;
        let pv: Vec<String> = self.best_pv.iter().map(|m| m.to_uci()).collect();
        send(&format!(
            "info depth {} seldepth {} multipv 1 score {} nodes {} nps {} hashfull {} time {} pv {}",
            depth,
            self.seldepth.max(depth as usize),
            score_string(score),
            nodes,
            nps,
            self.shared.tt.hashfull(),
            elapsed,
            pv.join(" ")
        ));
    }

    // ------------------------------------------------------------------
    // Hilfsfunktionen fuer die Zugsortierung
    // ------------------------------------------------------------------

    /// Vorheriger Zug (Figur, Zielfeld) und dazugehoeriger Countermove.
    #[inline]
    fn counter_info(&self, ply: usize) -> (Move, Option<(usize, usize)>) {
        let (pm, piece) = if ply > 0 {
            (self.stack[ply - 1].mv, self.stack[ply - 1].piece)
        } else {
            let lm = self.pos.last_move();
            if lm.is_none() || lm == Move::NULL {
                (Move::NONE, NO_PIECE)
            } else {
                (lm, self.pos.board[lm.to()])
            }
        };
        if pm.is_none() || pm == Move::NULL || piece == NO_PIECE {
            return (Move::NONE, None);
        }
        (self.td.countermoves[piece as usize][pm.to()], Some((piece as usize, pm.to())))
    }

    #[inline]
    fn score_move(&self, m: Move, tt_move: Move, killers: [Move; 2], counter: Move) -> i32 {
        if m == tt_move {
            return SCORE_TT;
        }
        if m.is_capture() {
            let victim = piece_type(self.pos.captured_piece(m));
            let attacker = piece_type(self.pos.board[m.from()]);
            let mut s = MVV[victim] * 16 - attacker as i32;
            if m.is_promotion() {
                if m.promotion_type() != QUEEN {
                    return SCORE_UNDERPROMO + s;
                }
                s += 800 * 16;
            }
            if self.pos.see_ge(m, 0) {
                SCORE_GOOD_CAPTURE + s
            } else {
                SCORE_BAD_CAPTURE + s
            }
        } else if m.is_promotion() {
            if m.promotion_type() == QUEEN {
                SCORE_QUEEN_PROMO
            } else {
                SCORE_UNDERPROMO
            }
        } else if m == killers[0] {
            SCORE_KILLER1
        } else if m == killers[1] {
            SCORE_KILLER2
        } else if m == counter {
            SCORE_COUNTER
        } else {
            self.td.history[self.pos.side][m.from()][m.to()]
        }
    }

    #[inline]
    fn update_history(&mut self, side: Color, m: Move, bonus: i32) {
        let h = &mut self.td.history[side][m.from()][m.to()];
        *h += bonus - *h * bonus.abs() / 16384;
    }

    // ------------------------------------------------------------------
    // Hauptsuche
    // ------------------------------------------------------------------

    fn negamax(&mut self, mut alpha: i32, mut beta: i32, mut depth: i32, ply: usize, cut_node: bool) -> i32 {
        self.pv_len[ply] = ply;
        if depth <= 0 {
            return self.qsearch(alpha, beta, ply);
        }
        let pv_node = beta - alpha > 1;
        let root = ply == 0;
        self.nodes += 1;
        if self.check_stop() {
            return 0;
        }
        if ply > self.seldepth {
            self.seldepth = ply;
        }
        let in_check = self.pos.in_check();

        if !root {
            if self.pos.is_draw(ply) {
                return 0;
            }
            if ply >= MAX_PLY - 1 {
                return if in_check { 0 } else { evaluate(&self.pos) };
            }
            // Mate-Distance-Pruning
            alpha = alpha.max(-MATE + ply as i32);
            beta = beta.min(MATE - ply as i32 - 1);
            if alpha >= beta {
                return alpha;
            }
        }

        // Transpositionstabelle
        let hash = self.pos.hash;
        let tte = self.shared.tt.probe(hash);
        let tt_move = tte.map_or(Move::NONE, |e| e.mv);
        if let Some(e) = tte {
            if !pv_node && e.depth >= depth && self.pos.halfmove < 90 {
                let s = score_from_tt(e.score, ply);
                if e.bound == BOUND_EXACT
                    || (e.bound == BOUND_LOWER && s >= beta)
                    || (e.bound == BOUND_UPPER && s <= alpha)
                {
                    return s;
                }
            }
        }

        // Statische Bewertung
        let static_eval;
        let mut eval;
        if in_check {
            static_eval = EVAL_NONE;
            eval = EVAL_NONE;
        } else {
            static_eval = match tte {
                Some(e) if e.eval != EVAL_NONE => e.eval,
                _ => evaluate(&self.pos),
            };
            eval = static_eval;
            if let Some(e) = tte {
                let s = score_from_tt(e.score, ply);
                if s.abs() < MATE_IN_MAX
                    && (e.bound == BOUND_EXACT
                        || (e.bound == BOUND_LOWER && s > eval)
                        || (e.bound == BOUND_UPPER && s < eval))
                {
                    eval = s;
                }
            }
        }
        self.stack[ply].static_eval = static_eval;
        let improving = if in_check {
            false
        } else if ply >= 2 && self.stack[ply - 2].static_eval != EVAL_NONE {
            static_eval > self.stack[ply - 2].static_eval
        } else if ply >= 4 && self.stack[ply - 4].static_eval != EVAL_NONE {
            static_eval > self.stack[ply - 4].static_eval
        } else {
            true
        };
        self.killers[ply + 2] = [Move::NONE; 2];

        if !pv_node && !in_check {
            // Reverse Futility Pruning
            // (nicht, wenn beta ein Mattwert ist - sonst werden kuerzere Matts uebersehen)
            let margin = 85 * depth - if improving { 60 } else { 0 };
            if depth <= 8 && eval.abs() < MATE_IN_MAX && beta.abs() < MATE_IN_MAX && eval - margin >= beta {
                return eval;
            }
            // Razoring
            if depth <= 3 && alpha.abs() < MATE_IN_MAX && eval + 250 * depth <= alpha {
                let v = self.qsearch(alpha, beta, ply);
                if self.stopped {
                    return 0;
                }
                if v <= alpha {
                    return v;
                }
            }
            // Null-Move-Pruning
            if depth >= 3
                && eval >= beta
                && (ply == 0 || self.stack[ply - 1].mv != Move::NULL)
                && self.pos.has_non_pawn_material(self.pos.side)
                && beta.abs() < MATE_IN_MAX
            {
                let r = 3 + depth / 3 + ((eval - beta) / 200).min(3);
                self.stack[ply].mv = Move::NULL;
                self.stack[ply].piece = NO_PIECE;
                self.pos.make_null_move();
                let s = -self.negamax(-beta, -beta + 1, depth - r, ply + 1, !cut_node);
                self.pos.unmake_null_move();
                if self.stopped {
                    return 0;
                }
                if s >= beta {
                    return if s >= MATE_IN_MAX { beta } else { s };
                }
            }
        }

        // Internal Iterative Reduction
        if depth >= 4 && tt_move.is_none() && (pv_node || cut_node) {
            depth -= 1;
        }

        // Zuege erzeugen und bewerten
        let mut list = MoveList::new();
        generate(&self.pos, &mut list, GenType::All);
        let n = list.len;
        let mut scores = [0i32; MAX_MOVES];
        let killers = self.killers[ply];
        let (counter, prev) = self.counter_info(ply);
        for i in 0..n {
            scores[i] = self.score_move(list.moves[i], tt_move, killers, counter);
        }

        let us = self.pos.side;
        let mut best_score = -INF;
        let mut best_move = Move::NONE;
        let mut legal = 0usize;
        let mut quiets_tried = [Move::NONE; 64];
        let mut nq = 0usize;
        let lmp_limit = if depth <= 8 {
            ((3 + depth * depth) / if improving { 1 } else { 2 }) as usize
        } else {
            usize::MAX
        };

        for i in 0..n {
            // Besten verbleibenden Zug nach vorne holen
            let mut bi = i;
            for j in i + 1..n {
                if scores[j] > scores[bi] {
                    bi = j;
                }
            }
            list.moves.swap(i, bi);
            scores.swap(i, bi);
            let m = list.moves[i];
            let ms = scores[i];
            let is_quiet = m.is_quiet();
            // `go searchmoves ...`: nur die erlaubten Wurzelzuege.
            if root && self.restrict_root && !self.root_moves.contains(&m) {
                continue;
            }

            // Kandidaten fuer das Auslassen schlechter Zuege (nie an der Wurzel,
            // nie im Schach, erst nachdem ein Zug durchsucht wurde).
            let prune = !root
                && !in_check
                && best_score > -MATE_IN_MAX
                && if is_quiet {
                    nq >= lmp_limit
                        || (depth <= 8 && alpha.abs() < MATE_IN_MAX && static_eval + 100 + 100 * depth <= alpha)
                        || (depth <= 8 && !self.pos.see_ge(m, -25 * depth * depth))
                } else {
                    depth <= 6 && !self.pos.see_ge(m, -100 * depth)
                };

            let piece = self.pos.board[m.from()];
            if !self.pos.make_move(m) {
                continue;
            }
            let gives_check = self.pos.in_check();
            // Schachgebende Zuege werden nie ausgelassen (koennten Matt sein).
            if prune && !gives_check {
                self.pos.unmake_move();
                continue;
            }
            legal += 1;
            self.stack[ply].mv = m;
            self.stack[ply].piece = piece;

            let ext = if gives_check && ply < 2 * self.root_depth as usize { 1 } else { 0 };
            let new_depth = depth - 1 + ext;

            let mut score;
            if legal == 1 {
                score = -self.negamax(-beta, -alpha, new_depth, ply + 1, !pv_node && !cut_node);
            } else {
                // Late Move Reductions
                let mut r = 0;
                if depth >= 3 && (is_quiet || ms < 0) {
                    r = LMR[(depth as usize).min(63)][legal.min(63)];
                    if pv_node {
                        r -= 1;
                    }
                    if cut_node {
                        r += 1;
                    }
                    if !improving {
                        r += 1;
                    }
                    if gives_check {
                        r -= 1;
                    }
                    if is_quiet {
                        if ms >= SCORE_COUNTER {
                            r -= 1;
                        } else {
                            r -= ms / 8192;
                        }
                    }
                    r = r.min(new_depth - 1).max(0);
                }
                score = -self.negamax(-alpha - 1, -alpha, new_depth - r, ply + 1, true);
                if score > alpha && r > 0 {
                    score = -self.negamax(-alpha - 1, -alpha, new_depth, ply + 1, !cut_node);
                }
                if pv_node && score > alpha && score < beta {
                    score = -self.negamax(-beta, -alpha, new_depth, ply + 1, false);
                }
            }
            self.pos.unmake_move();
            if self.stopped {
                return 0;
            }

            if score > best_score {
                best_score = score;
                if score > alpha {
                    best_move = m;
                    if pv_node {
                        self.pv[ply][ply] = m;
                        let child_len = self.pv_len[ply + 1];
                        for k in ply + 1..child_len {
                            self.pv[ply][k] = self.pv[ply + 1][k];
                        }
                        self.pv_len[ply] = child_len.max(ply + 1);
                    }
                    if score >= beta {
                        if is_quiet {
                            let bonus = (150 * depth - 100).clamp(0, 1600);
                            self.update_history(us, m, bonus);
                            for &q in &quiets_tried[..nq] {
                                self.update_history(us, q, -bonus);
                            }
                            if self.killers[ply][0] != m {
                                self.killers[ply][1] = self.killers[ply][0];
                                self.killers[ply][0] = m;
                            }
                            if let Some((p, to)) = prev {
                                self.td.countermoves[p][to] = m;
                            }
                        }
                        break;
                    }
                    alpha = score;
                }
            }
            if is_quiet && m != best_move && nq < quiets_tried.len() {
                quiets_tried[nq] = m;
                nq += 1;
            }
        }

        if legal == 0 {
            return if in_check { -MATE + ply as i32 } else { 0 };
        }

        let bound = if best_score >= beta {
            BOUND_LOWER
        } else if pv_node && !best_move.is_none() {
            BOUND_EXACT
        } else {
            BOUND_UPPER
        };
        self.shared.tt.store(hash, best_move, score_to_tt(best_score, ply), static_eval, depth, bound);
        best_score
    }

    // ------------------------------------------------------------------
    // Ruhesuche (nur Schlagzuege / Umwandlungen; im Schach alle Zuege)
    // ------------------------------------------------------------------

    fn qsearch(&mut self, mut alpha: i32, beta: i32, ply: usize) -> i32 {
        self.pv_len[ply] = ply;
        self.nodes += 1;
        if self.check_stop() {
            return 0;
        }
        if ply > self.seldepth {
            self.seldepth = ply;
        }
        let in_check = self.pos.in_check();
        if ply > 0 && self.pos.is_draw(ply) {
            return 0;
        }
        if ply >= MAX_PLY - 1 {
            return if in_check { 0 } else { evaluate(&self.pos) };
        }
        let pv_node = beta - alpha > 1;

        let hash = self.pos.hash;
        let tte = self.shared.tt.probe(hash);
        let tt_move = tte.map_or(Move::NONE, |e| e.mv);
        if let Some(e) = tte {
            if !pv_node {
                let s = score_from_tt(e.score, ply);
                if e.bound == BOUND_EXACT
                    || (e.bound == BOUND_LOWER && s >= beta)
                    || (e.bound == BOUND_UPPER && s <= alpha)
                {
                    return s;
                }
            }
        }

        let static_eval;
        let mut best_score;
        if in_check {
            static_eval = EVAL_NONE;
            best_score = -INF;
        } else {
            static_eval = match tte {
                Some(e) if e.eval != EVAL_NONE => e.eval,
                _ => evaluate(&self.pos),
            };
            best_score = static_eval;
            if let Some(e) = tte {
                let s = score_from_tt(e.score, ply);
                if s.abs() < MATE_IN_MAX
                    && (e.bound == BOUND_EXACT
                        || (e.bound == BOUND_LOWER && s > best_score)
                        || (e.bound == BOUND_UPPER && s < best_score))
                {
                    best_score = s;
                }
            }
            if best_score >= beta {
                return best_score;
            }
            if best_score > alpha {
                alpha = best_score;
            }
        }

        let mut list = MoveList::new();
        generate(&self.pos, &mut list, if in_check { GenType::All } else { GenType::Noisy });
        let n = list.len;
        let mut scores = [0i32; MAX_MOVES];
        for i in 0..n {
            let m = list.moves[i];
            scores[i] = if m == tt_move {
                SCORE_TT
            } else if m.is_capture() {
                let victim = piece_type(self.pos.captured_piece(m));
                let attacker = piece_type(self.pos.board[m.from()]);
                SCORE_GOOD_CAPTURE + MVV[victim] * 16 - attacker as i32
            } else if m.is_promotion() {
                if m.promotion_type() == QUEEN {
                    SCORE_QUEEN_PROMO
                } else {
                    SCORE_UNDERPROMO
                }
            } else {
                self.td.history[self.pos.side][m.from()][m.to()]
            };
        }

        let mut best_move = Move::NONE;
        let mut legal = 0;
        for i in 0..n {
            let mut bi = i;
            for j in i + 1..n {
                if scores[j] > scores[bi] {
                    bi = j;
                }
            }
            list.moves.swap(i, bi);
            scores.swap(i, bi);
            let m = list.moves[i];

            if !in_check {
                // Delta-Pruning: selbst der Gewinn der Figur reicht nicht.
                if !m.is_promotion() {
                    let gain = SEE_VALUES[piece_type(self.pos.captured_piece(m))];
                    if static_eval + gain + 200 <= alpha {
                        continue;
                    }
                }
                // Verlustreiche Schlagzuege auslassen.
                if !self.pos.see_ge(m, 0) {
                    continue;
                }
            }

            if !self.pos.make_move(m) {
                continue;
            }
            legal += 1;
            let s = -self.qsearch(-beta, -alpha, ply + 1);
            self.pos.unmake_move();
            if self.stopped {
                return 0;
            }
            if s > best_score {
                best_score = s;
                if s > alpha {
                    best_move = m;
                    if s >= beta {
                        break;
                    }
                    alpha = s;
                }
            }
        }

        if in_check && legal == 0 {
            return -MATE + ply as i32;
        }
        let bound = if best_score >= beta { BOUND_LOWER } else { BOUND_UPPER };
        self.shared.tt.store(hash, best_move, score_to_tt(best_score, ply), static_eval, 0, bound);
        best_score
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::movegen::legal_moves;
    use crate::time::UNTIMED;

    fn search_fen(pos: Position, depth: i32) -> (Move, i32, u64, Vec<Move>) {
        let shared = Arc::new(Shared::new(8));
        let mut p = pos.clone();
        let rm = legal_moves(&mut p);
        let limits = Limits { depth: Some(depth), silent: true, ..Default::default() };
        let mut w = Worker::new(0, 0, pos, ThreadData::new(), shared, limits, UNTIMED, rm.clone(), false);
        w.iterate();
        assert!(!w.best_move.is_none(), "kein Zug gefunden");
        assert!(rm.contains(&w.best_move), "bester Zug ist kein legaler Wurzelzug");
        (w.best_move, w.best_score, w.nodes(), w.best_pv.clone())
    }

    #[test]
    fn mate_in_one() {
        for (fen, mv) in [
            ("6k1/5ppp/8/8/8/8/5PPP/3R2K1 w - - 0 1", "d1d8"),
            ("r1bqkb1r/pppp1ppp/2n2n2/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR w KQkq - 4 4", "h5f7"),
        ] {
            let (m, s, _, _) = search_fen(Position::from_fen(fen).unwrap(), 5);
            assert_eq!(m.to_uci(), mv, "{}", fen);
            assert_eq!(s, MATE - 1, "{}", fen);
            assert_eq!(score_string(s), "mate 1");
        }
    }

    #[test]
    fn deterministic_single_thread() {
        let fen = "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10";
        let a = search_fen(Position::from_fen(fen).unwrap(), 8);
        let b = search_fen(Position::from_fen(fen).unwrap(), 8);
        assert_eq!((a.0, a.1, a.2), (b.0, b.1, b.2));
    }

    /// PV muss eine legale Zugfolge sein.
    fn check_pv(pos: &Position, pv: &[Move]) {
        let mut p = pos.clone();
        for &m in pv {
            assert!(legal_moves(&mut p).contains(&m), "PV enthaelt illegalen Zug {}", m.to_uci());
            assert!(p.make_move(m));
        }
    }

    /// Zufallspartien: auf jeder Stellung kurz suchen (Absturz-/Ueberlauf-Test).
    #[test]
    fn fuzz_random_positions() {
        let mut seed: u64 = 0xDEAD_BEEF_CAFE_F00D;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _game in 0..40 {
            let mut pos = Position::startpos();
            for _ply in 0..160 {
                let moves = legal_moves(&mut pos);
                if moves.is_empty() || pos.is_insufficient_material() || pos.halfmove >= 100 {
                    break;
                }
                if rnd() % 8 == 0 {
                    let (_, s, _, pv) = search_fen(pos.clone(), 4);
                    assert!(s.abs() <= MATE, "Wert ausserhalb des Bereichs: {}", s);
                    check_pv(&pos, &pv);
                }
                let m = moves[(rnd() % moves.len() as u64) as usize];
                assert!(pos.make_move(m));
            }
        }
    }
}
