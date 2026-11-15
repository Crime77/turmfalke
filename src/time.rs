//! Suchgrenzen aus dem UCI-Befehl `go` und Zeitplanung.

use crate::types::Color;
use std::sync::LazyLock;
use std::time::Instant;

static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Millisekunden seit Programmstart (monoton).
pub fn now_ms() -> u64 {
    EPOCH.elapsed().as_millis() as u64
}

#[derive(Clone, Debug, Default)]
pub struct Limits {
    /// Restzeit [weiss, schwarz] in ms.
    pub time: [Option<u64>; 2],
    /// Inkrement [weiss, schwarz] in ms.
    pub inc: [u64; 2],
    pub movestogo: Option<u64>,
    pub movetime: Option<u64>,
    pub depth: Option<i32>,
    pub nodes: Option<u64>,
    pub mate: Option<i32>,
    pub infinite: bool,
    pub ponder: bool,
    /// Es kam mindestens ein Uhr-Parameter (wtime/btime/winc/binc/movestogo).
    pub has_clock: bool,
    /// `searchmoves`: erlaubte Wurzelzuege in UCI-Schreibweise.
    pub searchmoves: Vec<String>,
    /// Keine info-Ausgaben (fuer bench).
    pub silent: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct TimePlan {
    /// Mit Zeitlimit?
    pub timed: bool,
    /// Feste Zugzeit (`movetime`): nur die harte Grenze zaehlt.
    pub fixed: bool,
    /// Weiche Grenze: keine neue Iteration mehr beginnen.
    pub soft: u64,
    /// Harte Grenze: Suche sofort abbrechen.
    pub hard: u64,
}

pub const UNTIMED: TimePlan = TimePlan { timed: false, fixed: false, soft: u64::MAX, hard: u64::MAX };

/// Angenommene Restzeit, wenn Uhr-Parameter kamen, aber die eigene Zeit
/// fehlt oder nicht lesbar war (lieber schnell ziehen als unbegrenzt suchen).
const FALLBACK_TIME_MS: u64 = 1000;

/// Plant die Zeit fuer einen Zug der Seite `side`.
pub fn plan(l: &Limits, side: Color, overhead: u64) -> TimePlan {
    if l.infinite {
        return UNTIMED;
    }
    if let Some(mt) = l.movetime {
        let t = mt.saturating_sub(overhead).max(1);
        return TimePlan { timed: true, fixed: true, soft: t, hard: t };
    }
    let own_time = match l.time[side] {
        Some(t) => Some(t),
        None if l.has_clock => Some(FALLBACK_TIME_MS),
        None => None,
    };
    if let Some(time) = own_time {
        let inc = l.inc[side];
        let avail = time.saturating_sub(overhead).max(1);
        let mtg = l.movestogo.unwrap_or(30).clamp(1, 50);
        let base = avail / mtg + inc * 3 / 4;
        // Nie mehr als die Haelfte (weich) bzw. drei Viertel (hart) der Restzeit.
        let soft = base.min(avail / 2).max(1);
        let hard = (base * 3).min(avail * 3 / 4).max(soft);
        return TimePlan { timed: true, fixed: false, soft, hard };
    }
    UNTIMED
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{BLACK, WHITE};

    #[test]
    fn never_overspends() {
        for &time in &[1u64, 10, 50, 100, 1000, 10_000, 600_000] {
            for &inc in &[0u64, 10, 100, 2000] {
                for mtg in [None, Some(1), Some(5), Some(40)] {
                    let l = Limits { time: [Some(time), Some(time)], inc: [inc, inc], movestogo: mtg, ..Default::default() };
                    for side in [WHITE, BLACK] {
                        let p = plan(&l, side, 30);
                        assert!(p.timed && p.soft >= 1 && p.soft <= p.hard);
                        let avail = time.saturating_sub(30).max(1);
                        assert!(p.hard <= avail.max(1), "hard {} > avail {} (time {}, inc {})", p.hard, avail, time, inc);
                    }
                }
            }
        }
    }

    #[test]
    fn missing_own_clock_is_limited() {
        // Nur die Zeit von Weiss bekannt, Schwarz am Zug: trotzdem begrenzt.
        let l = Limits { time: [Some(5000), None], has_clock: true, ..Default::default() };
        let p = plan(&l, BLACK, 30);
        assert!(p.timed && p.hard <= FALLBACK_TIME_MS);
        // movetime ist fest
        let l = Limits { movetime: Some(1000), ..Default::default() };
        let p = plan(&l, WHITE, 30);
        assert!(p.timed && p.fixed && p.hard == 970);
        // ohne Angaben: unbegrenzt
        assert!(!plan(&Limits::default(), WHITE, 30).timed);
    }
}
