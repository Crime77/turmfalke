//! UCI-Protokoll: liest Befehle von stdin und steuert die Suche.

use crate::eval::evaluate;
use crate::movegen::legal_moves;
use crate::perft;
use crate::position::Position;
use crate::search::{score_string, send, Shared, ThreadData, Worker, MAX_THREADS};
use crate::smp::{Job, Pool};
use crate::time::{self, now_ms, Limits, UNTIMED};
use crate::types::*;
use std::io::BufRead;
use std::sync::atomic::Ordering::{Relaxed, SeqCst};
use std::sync::Arc;

pub const NAME: &str = "Turmfalke 1.0";
const DEFAULT_HASH: usize = 256;
const MAX_HASH: usize = 65536;
const DEFAULT_OVERHEAD: u64 = 30;

/// Anzahl logischer CPU-Kerne (mehr Such-Threads waeren nur langsamer).
fn cpu_threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, MAX_THREADS)
}

fn default_threads() -> usize {
    cpu_threads().min(8)
}

pub struct Uci {
    pos: Position,
    /// false nach ungueltiger FEN oder illegalem Zug in `position`.
    pos_valid: bool,
    pool: Option<Pool>,
    threads: usize,
    overhead: u64,
    clear_epoch: u64,
}

impl Uci {
    pub fn new() -> Uci {
        let threads = default_threads();
        Uci {
            pos: Position::startpos(),
            pos_valid: true,
            pool: Some(Pool::new(Arc::new(Shared::new(DEFAULT_HASH)), threads)),
            threads,
            overhead: DEFAULT_OVERHEAD,
            clear_epoch: 0,
        }
    }

    fn pool(&self) -> &Pool {
        self.pool.as_ref().expect("Thread-Pool vorhanden")
    }

    fn shared(&self) -> &Arc<Shared> {
        self.pool().shared()
    }

    /// Bricht eine laufende Suche ab und wartet, bis ihr bestmove gesendet ist.
    fn stop_search(&mut self) {
        if let Some(pool) = self.pool.as_ref() {
            pool.stop();
            pool.wait_main();
        }
    }

    /// Thread-Pool neu aufbauen (neue Thread-Anzahl und/oder Hash-Groesse).
    fn rebuild(&mut self, new_hash_mb: Option<usize>) {
        self.stop_search();
        let pool = self.pool.take().expect("Thread-Pool vorhanden");
        let mut shared = pool.shutdown();
        if let Some(mb) = new_hash_mb {
            match Arc::get_mut(&mut shared) {
                Some(sh) => {
                    if !sh.tt.resize(mb) {
                        send("info string Nicht genug Speicher fuer diese Hash-Groesse, nutze 16 MB");
                    }
                }
                None => {
                    drop(shared);
                    shared = Arc::new(Shared::new(mb));
                }
            }
        }
        let pool = Pool::new(shared, self.threads);
        if pool.threads() < self.threads {
            send(&format!("info string Nur {} Such-Threads konnten gestartet werden", pool.threads()));
        }
        self.pool = Some(pool);
    }

    pub fn handle(&mut self, line: &str) -> bool {
        self.handle_at(line, now_ms())
    }

    /// Verarbeitet eine Zeile, die zum Zeitpunkt `received` (ms) eingelesen
    /// wurde. Liefert false bei `quit`.
    pub fn handle_at(&mut self, line: &str, received: u64) -> bool {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let Some(&cmd) = tokens.first() else {
            return true;
        };
        match cmd {
            "uci" => {
                send(&format!("id name {}", NAME));
                send("id author Claude");
                send(&format!("option name Hash type spin default {} min 1 max {}", DEFAULT_HASH, MAX_HASH));
                send(&format!(
                    "option name Threads type spin default {} min 1 max {}",
                    default_threads(),
                    MAX_THREADS
                ));
                send(&format!("option name Move Overhead type spin default {} min 0 max 5000", DEFAULT_OVERHEAD));
                send("option name Clear Hash type button");
                send("option name Ponder type check default false");
                send("uciok");
            }
            "isready" => send("readyok"),
            "ucinewgame" => {
                self.stop_search();
                self.pool().wait_idle();
                self.shared().tt.clear();
                self.clear_epoch += 1;
                self.pos = Position::startpos();
                self.pos_valid = true;
            }
            "setoption" => self.cmd_setoption(&tokens),
            "position" => {
                self.stop_search();
                self.cmd_position(&tokens);
            }
            "go" => {
                self.stop_search();
                self.cmd_go(&tokens, received);
            }
            "stop" => self.stop_search(),
            "ponderhit" => {
                let sh = self.shared();
                if sh.pondering.load(SeqCst) {
                    sh.start_ms.store(received, SeqCst);
                    sh.pondering.store(false, SeqCst);
                }
            }
            "quit" => {
                self.stop_search();
                if let Some(pool) = self.pool.take() {
                    pool.shutdown();
                }
                return false;
            }
            "d" => send(&self.pos.pretty()),
            "eval" => send(&format!(
                "info string Statische Bewertung (Sicht Seite am Zug): {} cp",
                evaluate(&self.pos)
            )),
            "perft" => {
                self.stop_search();
                let depth = tokens.get(1).and_then(|s| s.parse::<u32>().ok()).unwrap_or(1).min(10);
                let start = now_ms();
                let mut p = self.pos.clone();
                let (list, total) = perft::divide(&mut p, depth);
                for (m, n) in list {
                    send(&format!("{}: {}", m.to_uci(), n));
                }
                let ms = now_ms().saturating_sub(start).max(1);
                send(&format!("\nNodes searched: {}\nZeit: {} ms ({} Knoten/s)", total, ms, total * 1000 / ms));
            }
            "bench" => {
                self.stop_search();
                let depth = tokens.get(1).and_then(|s| s.parse::<i32>().ok()).unwrap_or(12).clamp(1, 30);
                bench(depth);
            }
            "debug" | "register" => {}
            _ => send(&format!("info string Unbekannter Befehl: {}", line.trim())),
        }
        true
    }

    fn cmd_setoption(&mut self, tokens: &[&str]) {
        let name_idx = tokens.iter().position(|t| t.eq_ignore_ascii_case("name"));
        let value_idx = tokens.iter().position(|t| t.eq_ignore_ascii_case("value"));
        let Some(ni) = name_idx else {
            send("info string setoption: 'name' fehlt");
            return;
        };
        let name_end = value_idx.filter(|&v| v > ni).unwrap_or(tokens.len());
        let name = tokens[ni + 1..name_end].join(" ").to_ascii_lowercase();
        let value = value_idx.map(|v| tokens[v + 1..].join(" ")).unwrap_or_default();
        let num = parse_number(value.trim());
        match name.as_str() {
            "hash" => match num {
                Some(v) => self.rebuild(Some(v.clamp(1, MAX_HASH as i64) as usize)),
                None => send("info string Hash: Zahl erwartet"),
            },
            "threads" => match num {
                Some(v) => {
                    let wanted = v.clamp(1, MAX_THREADS as i64) as usize;
                    let threads = wanted.min(cpu_threads());
                    if threads < wanted {
                        send(&format!(
                            "info string Threads auf {} begrenzt (Anzahl der CPU-Kerne)",
                            threads
                        ));
                    }
                    if threads != self.threads {
                        self.threads = threads;
                        self.rebuild(None);
                    }
                }
                None => send("info string Threads: Zahl erwartet"),
            },
            "move overhead" => match num {
                Some(v) => self.overhead = v.clamp(0, 5000) as u64,
                None => send("info string Move Overhead: Zahl erwartet"),
            },
            "clear hash" => {
                self.stop_search();
                self.pool().wait_idle();
                self.shared().tt.clear();
            }
            "ponder" => {}
            _ => send(&format!("info string Unbekannte Option: {}", name)),
        }
    }

    fn cmd_position(&mut self, tokens: &[&str]) {
        let moves_idx = tokens.iter().position(|&t| t == "moves");
        let base = match tokens.get(1) {
            Some(&"startpos") => Ok(Position::startpos()),
            Some(&"fen") => {
                let end = moves_idx.unwrap_or(tokens.len()).max(2);
                Position::from_fen(&tokens[2..end].join(" "))
            }
            _ => Err("position: 'startpos' oder 'fen' erwartet".to_string()),
        };
        let mut p = match base {
            Ok(p) => p,
            Err(e) => {
                send(&format!("info string Fehler: {}", e));
                self.pos_valid = false;
                return;
            }
        };
        if let Some(mi) = moves_idx {
            for mv in &tokens[mi + 1..] {
                match p.parse_uci_move(mv) {
                    Some(m) => {
                        p.make_move(m);
                    }
                    None => {
                        send(&format!("info string Fehler: illegaler Zug '{}' in position", mv));
                        self.pos_valid = false;
                        return;
                    }
                }
            }
        }
        self.pos = p;
        self.pos_valid = true;
    }

    fn cmd_go(&mut self, tokens: &[&str], received: u64) {
        let limits = parse_go(tokens);
        if !self.pos_valid {
            send("info string Fehler: keine gueltige Stellung gesetzt");
            send("bestmove 0000");
            return;
        }
        let plan = time::plan(&limits, self.pos.side, self.overhead);
        let mut p = self.pos.clone();
        let mut root_moves = legal_moves(&mut p);
        let mut restrict_root = false;
        if !limits.searchmoves.is_empty() {
            let filtered: Vec<Move> =
                root_moves.iter().copied().filter(|m| limits.searchmoves.contains(&m.to_uci())).collect();
            if !filtered.is_empty() {
                root_moves = filtered;
                restrict_root = true;
            }
        }
        let job = Job {
            pos: self.pos.clone(),
            limits,
            plan,
            root_moves,
            restrict_root,
            start_ms: received,
            clear_epoch: self.clear_epoch,
        };
        if let Some(pool) = self.pool.as_mut() {
            pool.start(job);
        }
    }
}

/// Zahl lesen; Kommazahlen werden abgerundet (manche GUIs senden "1000.5").
fn parse_number(s: &str) -> Option<i64> {
    if let Ok(v) = s.parse::<i64>() {
        return Some(v);
    }
    match s.parse::<f64>() {
        Ok(f) if f.is_finite() => Some(f.clamp(-1e15, 1e15) as i64),
        _ => None,
    }
}

const GO_KEYWORDS: [&str; 12] = [
    "wtime", "btime", "winc", "binc", "movestogo", "movetime", "depth", "nodes", "mate", "infinite", "ponder",
    "searchmoves",
];

fn parse_go(tokens: &[&str]) -> Limits {
    let mut l = Limits::default();
    let mut i = 1;
    let num = |i: usize| tokens.get(i).and_then(|s| parse_number(s));
    while i < tokens.len() {
        match tokens[i] {
            "wtime" => {
                l.has_clock = true;
                l.time[WHITE] = num(i + 1).map(|v| v.max(0) as u64);
                i += 1;
            }
            "btime" => {
                l.has_clock = true;
                l.time[BLACK] = num(i + 1).map(|v| v.max(0) as u64);
                i += 1;
            }
            "winc" => {
                l.has_clock = true;
                l.inc[WHITE] = num(i + 1).unwrap_or(0).max(0) as u64;
                i += 1;
            }
            "binc" => {
                l.has_clock = true;
                l.inc[BLACK] = num(i + 1).unwrap_or(0).max(0) as u64;
                i += 1;
            }
            "movestogo" => {
                l.has_clock = true;
                l.movestogo = num(i + 1).filter(|&v| v > 0).map(|v| v as u64);
                i += 1;
            }
            "movetime" => {
                l.movetime = num(i + 1).map(|v| v.max(0) as u64);
                i += 1;
            }
            "depth" => {
                l.depth = num(i + 1).map(|v| v.clamp(1, 127) as i32);
                i += 1;
            }
            "nodes" => {
                l.nodes = num(i + 1).map(|v| v.max(1) as u64);
                i += 1;
            }
            "mate" => {
                l.mate = num(i + 1).map(|v| v.clamp(1, 60) as i32);
                i += 1;
            }
            "infinite" => l.infinite = true,
            "ponder" => l.ponder = true,
            "searchmoves" => {
                while i + 1 < tokens.len() && !GO_KEYWORDS.contains(&tokens[i + 1]) {
                    l.searchmoves.push(tokens[i + 1].to_ascii_lowercase());
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    l
}

const BENCH_FENS: [&str; 10] = [
    "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
    "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
    "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1",
    "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1",
    "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8",
    "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10",
    "r1bqkb1r/pppp1ppp/2n2n2/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR w KQkq - 4 4",
    "6k1/5ppp/8/8/8/8/5PPP/3R2K1 w - - 0 1",
    "8/8/8/4k3/8/8/2K5/3Q4 w - - 0 1",
    "2r3k1/pp3ppp/2n5/3p4/3P4/2P2N2/PP3PPP/R5K1 b - - 0 20",
];

/// Feste Suche auf mehreren Stellungen (Geschwindigkeit + Determinismus).
pub fn bench(depth: i32) {
    let shared = Arc::new(Shared::new(16));
    let mut total = 0u64;
    let start = now_ms();
    for fen in BENCH_FENS {
        let pos = match Position::from_fen(fen) {
            Ok(p) => p,
            Err(e) => {
                send(&format!("info string Bench-FEN ungueltig ({}): {}", e, fen));
                continue;
            }
        };
        shared.tt.clear();
        shared.stop.store(false, Relaxed);
        shared.uci_stop.store(false, Relaxed);
        shared.pondering.store(false, Relaxed);
        shared.start_ms.store(now_ms(), Relaxed);
        for c in &shared.nodes {
            c.store(0, Relaxed);
        }
        let mut p = pos.clone();
        let root_moves = legal_moves(&mut p);
        let limits = Limits { depth: Some(depth), silent: true, ..Default::default() };
        let gen = shared.generation.load(Relaxed);
        let mut w = Worker::new(0, gen, pos, ThreadData::new(), shared.clone(), limits, UNTIMED, root_moves, false);
        w.iterate();
        total += w.nodes();
        send(&format!(
            "info string {:<72} bestmove {:<6} score {}",
            fen,
            w.final_best_move().to_uci(),
            score_string(w.best_score)
        ));
    }
    let ms = now_ms().saturating_sub(start).max(1);
    send(&format!("Zeit: {} ms", ms));
    send(&format!("{} nodes {} nps", total, total * 1000 / ms));
}

/// Hauptschleife: liest stdin zeilenweise (auch bei ungueltigem UTF-8 robust).
/// Der Zeitstempel wird direkt beim Einlesen genommen, damit die Zeitmessung
/// fuer `go` alles ab Empfang beruecksichtigt.
pub fn run() {
    let mut uci = Uci::new();
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break, // EOF
            Ok(_) => {
                let received = now_ms();
                let line = String::from_utf8_lossy(&buf);
                if !uci.handle_at(line.trim(), received) {
                    return;
                }
            }
        }
    }
    uci.handle("quit");
}
