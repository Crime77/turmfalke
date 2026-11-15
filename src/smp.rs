//! Lazy SMP mit dauerhaftem Thread-Pool.
//!
//! Die Such-Threads werden einmal gestartet und warten zwischen den Zuegen
//! auf eine Condvar. So liegen weder das Erzeugen noch das Beenden von
//! Threads auf dem zeitkritischen Weg. Thread 0 steuert die Zeit, gibt
//! `info` aus und sendet `bestmove` sofort - ohne auf die Helfer zu warten.
//! Helfer einer alten Suche erkennen an der Generationsnummer, dass sie
//! veraltet sind, und brechen ab.

use crate::position::Position;
use crate::search::{send, Shared, ThreadData, Worker};
use crate::time::{Limits, TimePlan};
use crate::types::Move;
use std::sync::atomic::Ordering::{Relaxed, SeqCst};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};

const STACK_SIZE: usize = 32 * 1024 * 1024;

/// Unter dieser harten Zeitgrenze (ms) sucht nur Thread 0. So knappe Suchen
/// gewinnen durch Helfer kaum Staerke, und bei ausgelasteter CPU bekommt der
/// zeitkritische Haupt-Thread so einen Kern fuer sich (Schutz vor Zeitverlust).
const SOLO_BELOW_MS: u64 = 200;

/// Laufen die Helfer bei diesem Auftrag mit?
fn helpers_wanted(job: &Job) -> bool {
    let tight = job.plan.timed && job.plan.hard < SOLO_BELOW_MS;
    !tight || job.limits.ponder || job.limits.infinite
}

/// Ein Suchauftrag.
pub struct Job {
    pub pos: Position,
    pub limits: Limits,
    pub plan: TimePlan,
    /// Legale Wurzelzuege (bei `searchmoves` bereits gefiltert).
    pub root_moves: Vec<Move>,
    /// Nur die Zuege aus root_moves durchsuchen (`searchmoves`).
    pub restrict_root: bool,
    /// Zeitpunkt, zu dem `go` empfangen wurde (ms).
    pub start_ms: u64,
    /// Aendert sich bei `ucinewgame`: Lerntabellen zuruecksetzen.
    pub clear_epoch: u64,
}

struct State {
    generation: u64,
    job: Option<Arc<Job>>,
    quit: bool,
    /// Je Thread: letzte vollstaendig bearbeitete Generation.
    done: Vec<u64>,
}

struct Sync {
    state: Mutex<State>,
    cv: Condvar,
}

impl Sync {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

pub struct Pool {
    shared: Arc<Shared>,
    sync: Arc<Sync>,
    handles: Vec<JoinHandle<()>>,
    generation: u64,
}

impl Pool {
    pub fn new(shared: Arc<Shared>, threads: usize) -> Pool {
        let sync = Arc::new(Sync {
            state: Mutex::new(State { generation: 0, job: None, quit: false, done: vec![0; threads.max(1)] }),
            cv: Condvar::new(),
        });
        let mut handles = Vec::new();
        for id in 0..threads.max(1) {
            let sh = shared.clone();
            let sy = sync.clone();
            match thread::Builder::new()
                .name(format!("turmfalke-{}", id))
                .stack_size(STACK_SIZE)
                .spawn(move || worker_loop(id, sh, sy))
            {
                Ok(h) => handles.push(h),
                Err(_) => break, // weniger Threads als gewuenscht - trotzdem lauffaehig
            }
        }
        // Nur tatsaechlich laufende Threads zaehlen (ids 0..handles.len()).
        sync.lock().done.truncate(handles.len());
        Pool { shared, sync, handles, generation: 0 }
    }

    pub fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    pub fn threads(&self) -> usize {
        self.handles.len()
    }

    /// Startet eine neue Suche. Vorher muss die alte mit stop() + wait_main()
    /// beendet worden sein (sonst ginge deren bestmove verloren).
    pub fn start(&mut self, job: Job) {
        let sh = &self.shared;
        if self.handles.is_empty() {
            // Notfall: kein Such-Thread verfuegbar -> sofort legalen Zug melden.
            let mv = job.root_moves.first().copied().unwrap_or(Move::NONE);
            send(&format!("bestmove {}", mv.to_uci()));
            return;
        }
        self.generation += 1;
        sh.stop.store(false, SeqCst);
        sh.uci_stop.store(false, SeqCst);
        sh.start_ms.store(job.start_ms, SeqCst);
        sh.pondering.store(job.limits.ponder, SeqCst);
        for c in &sh.nodes {
            c.store(0, Relaxed);
        }
        sh.tt.new_search();
        sh.generation.store(self.generation, SeqCst);
        {
            let mut st = self.sync.lock();
            st.generation = self.generation;
            st.job = Some(Arc::new(job));
        }
        self.sync.cv.notify_all();
    }

    /// Bricht eine laufende Suche ab (blockiert nicht).
    pub fn stop(&self) {
        self.shared.stop.store(true, SeqCst);
        self.shared.uci_stop.store(true, SeqCst);
        self.shared.pondering.store(false, SeqCst);
    }

    /// Wartet, bis Thread 0 die aktuelle Suche beendet (bestmove gesendet) hat.
    pub fn wait_main(&self) {
        if self.handles.is_empty() {
            return;
        }
        let mut st = self.sync.lock();
        while st.done[0] != st.generation {
            st = self.sync.cv.wait(st).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Wartet, bis alle Threads die aktuelle Suche beendet haben.
    pub fn wait_idle(&self) {
        let mut st = self.sync.lock();
        while st.done.iter().any(|&d| d != st.generation) {
            st = self.sync.cv.wait(st).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Beendet alle Threads und gibt den geteilten Zustand zurueck.
    pub fn shutdown(self) -> Arc<Shared> {
        self.stop();
        {
            let mut st = self.sync.lock();
            st.quit = true;
        }
        self.sync.cv.notify_all();
        for h in self.handles {
            let _ = h.join();
        }
        self.shared
    }
}

fn worker_loop(id: usize, shared: Arc<Shared>, sync: Arc<Sync>) {
    let mut td: Option<Box<ThreadData>> = Some(ThreadData::new());
    let mut clear_epoch = 0u64;
    let mut seen = 0u64;
    loop {
        let job = {
            let mut st = sync.lock();
            loop {
                if st.quit {
                    return;
                }
                if st.generation != seen {
                    break;
                }
                st = sync.cv.wait(st).unwrap_or_else(|e| e.into_inner());
            }
            seen = st.generation;
            st.job.clone()
        };
        if let Some(job) = job {
            if job.clear_epoch != clear_epoch {
                td = Some(ThreadData::new());
                clear_epoch = job.clear_epoch;
            }
            let t = td.take().unwrap_or_else(ThreadData::new);
            td = Some(if id == 0 { run_main(&shared, &job, seen, t) } else { run_helper(id, &shared, &job, seen, t) });
        }
        {
            let mut st = sync.lock();
            if id < st.done.len() {
                st.done[id] = seen;
            }
        }
        sync.cv.notify_all();
    }
}

/// Bei `go infinite` / `go ponder`: erst nach stop bzw. ponderhit antworten.
fn wait_for_release(shared: &Shared, limits: &Limits, generation: u64) {
    while !shared.uci_stop.load(SeqCst)
        && shared.generation.load(SeqCst) == generation
        && (limits.infinite || shared.pondering.load(SeqCst))
    {
        thread::sleep(std::time::Duration::from_millis(1));
    }
}

fn run_main(shared: &Arc<Shared>, job: &Job, generation: u64, td: Box<ThreadData>) -> Box<ThreadData> {
    if job.root_moves.is_empty() {
        // Matt oder Patt: kein Zug moeglich.
        if !job.limits.silent {
            let score = if job.pos.in_check() { "mate 0" } else { "cp 0" };
            send(&format!("info depth 0 score {}", score));
        }
        wait_for_release(shared, &job.limits, generation);
        send("bestmove 0000");
        return td;
    }
    let mut w = Worker::new(
        0,
        generation,
        job.pos.clone(),
        td,
        shared.clone(),
        job.limits.clone(),
        job.plan,
        job.root_moves.clone(),
        job.restrict_root,
    );
    w.iterate();
    // Helfer anhalten; bestmove sofort senden, ohne auf sie zu warten.
    shared.stop.store(true, SeqCst);
    let best = w.final_best_move();
    match w.ponder_move() {
        Some(p) => send(&format!("bestmove {} ponder {}", best.to_uci(), p.to_uci())),
        None => send(&format!("bestmove {}", best.to_uci())),
    }
    w.td
}

fn run_helper(id: usize, shared: &Arc<Shared>, job: &Job, generation: u64, td: Box<ThreadData>) -> Box<ThreadData> {
    if job.root_moves.is_empty() || !helpers_wanted(job) {
        return td;
    }
    let mut w = Worker::new(
        id,
        generation,
        job.pos.clone(),
        td,
        shared.clone(),
        job.limits.clone(),
        job.plan,
        job.root_moves.clone(),
        job.restrict_root,
    );
    w.iterate();
    w.td
}
