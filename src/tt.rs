//! Transpositionstabelle: lock-frei und thread-sicher (Lazy SMP).
//!
//! Jeder Eintrag besteht aus zwei atomaren 64-Bit-Woertern: `key ^ data`
//! und `data`. Ein von einem anderen Thread halb ueberschriebener Eintrag
//! faellt beim Lesen durch die XOR-Pruefung auf und wird ignoriert.

use crate::types::Move;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};

pub const BOUND_UPPER: u8 = 1;
pub const BOUND_LOWER: u8 = 2;
pub const BOUND_EXACT: u8 = 3;

#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub mv: Move,
    pub score: i32,
    pub eval: i32,
    pub depth: i32,
    pub bound: u8,
}

#[derive(Default)]
struct Slot {
    key: AtomicU64,
    data: AtomicU64,
}

#[repr(align(64))]
#[derive(Default)]
struct Bucket {
    slots: [Slot; 4],
}

pub struct TT {
    buckets: Vec<Bucket>,
    age: AtomicU32,
}

#[inline(always)]
fn pack(mv: Move, score: i32, eval: i32, depth: i32, bound: u8, age: u32) -> u64 {
    (mv.0 as u64)
        | ((score.clamp(-32767, 32767) as i16 as u16 as u64) << 16)
        | ((eval.clamp(-32768, 32767) as i16 as u16 as u64) << 32)
        | ((depth.clamp(0, 255) as u64) << 48)
        | (((bound & 3) as u64) << 56)
        | (((age & 63) as u64) << 58)
}

#[inline(always)]
fn unpack(d: u64) -> (Entry, u32) {
    (
        Entry {
            mv: Move(d as u16),
            score: (d >> 16) as u16 as i16 as i32,
            eval: (d >> 32) as u16 as i16 as i32,
            depth: ((d >> 48) & 255) as i32,
            bound: ((d >> 56) & 3) as u8,
        },
        ((d >> 58) & 63) as u32,
    )
}

/// Holt genullten Speicher direkt vom Betriebssystem. Das ist auch bei
/// vielen GB sofort fertig, weil die Seiten erst bei Benutzung angelegt werden.
fn allocate(mb: usize) -> Option<Vec<Bucket>> {
    let n = (mb.max(1) * 1024 * 1024 / std::mem::size_of::<Bucket>()).max(1);
    let layout = std::alloc::Layout::array::<Bucket>(n).ok()?;
    // SAFETY: `Bucket` besteht nur aus AtomicU64-Feldern, fuer die das
    // Null-Bitmuster ein gueltiger Wert ist. Groesse > 0 (n >= 1). Der Speicher
    // kommt vom globalen Allokator mit genau dem Layout, das ein Vec<Bucket>
    // mit Kapazitaet n beim Freigeben verwendet (Layout::array::<Bucket>(n)).
    unsafe {
        let ptr = std::alloc::alloc_zeroed(layout) as *mut Bucket;
        if ptr.is_null() {
            return None;
        }
        Some(Vec::from_raw_parts(ptr, n, n))
    }
}

impl TT {
    pub fn new(mb: usize) -> TT {
        let buckets = allocate(mb).or_else(|| allocate(16)).expect("Speicher fuer Hash-Tabelle");
        TT { buckets, age: AtomicU32::new(0) }
    }

    /// Neue Groesse in MB. Liefert false, wenn der Speicher nicht reicht
    /// (dann wird eine kleine Tabelle mit 16 MB benutzt).
    pub fn resize(&mut self, mb: usize) -> bool {
        self.buckets = Vec::new(); // alten Speicher zuerst freigeben
        match allocate(mb) {
            Some(b) => {
                self.buckets = b;
                true
            }
            None => {
                self.buckets = allocate(16).expect("Speicher fuer Hash-Tabelle");
                false
            }
        }
    }

    pub fn clear(&self) {
        let chunks = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 64);
        let len = self.buckets.len();
        let per = len.div_ceil(chunks);
        std::thread::scope(|s| {
            for part in self.buckets.chunks(per.max(1)) {
                s.spawn(move || {
                    for b in part {
                        for slot in &b.slots {
                            slot.key.store(0, Relaxed);
                            slot.data.store(0, Relaxed);
                        }
                    }
                });
            }
        });
        self.age.store(0, Relaxed);
    }

    /// Vor jeder neuen Suche aufrufen (Alterung alter Eintraege).
    pub fn new_search(&self) {
        let a = self.age.load(Relaxed);
        self.age.store((a + 1) & 63, Relaxed);
    }

    #[inline(always)]
    fn bucket(&self, hash: u64) -> &Bucket {
        let idx = ((hash as u128 * self.buckets.len() as u128) >> 64) as usize;
        &self.buckets[idx]
    }

    #[inline]
    pub fn probe(&self, hash: u64) -> Option<Entry> {
        for slot in &self.bucket(hash).slots {
            let d = slot.data.load(Relaxed);
            let k = slot.key.load(Relaxed);
            if d != 0 && k ^ d == hash {
                return Some(unpack(d).0);
            }
        }
        None
    }

    /// Speichert einen Eintrag. `score` muss bereits fuer die TT korrigiert sein
    /// (Mattwerte relativ zur Stellung).
    #[inline]
    pub fn store(&self, hash: u64, mut mv: Move, score: i32, eval: i32, depth: i32, bound: u8) {
        let age = self.age.load(Relaxed) & 63;
        let bucket = self.bucket(hash);
        let mut target = 0usize;
        let mut target_val = i32::MAX;
        for (i, slot) in bucket.slots.iter().enumerate() {
            let d = slot.data.load(Relaxed);
            let k = slot.key.load(Relaxed);
            if d == 0 {
                if target_val > i32::MIN {
                    target = i;
                    target_val = i32::MIN;
                }
                continue;
            }
            let (e, e_age) = unpack(d);
            if k ^ d == hash {
                // Gleiche Stellung: tiefere Information der aktuellen Suche behalten.
                if bound != BOUND_EXACT && e_age == age && depth + 3 < e.depth {
                    return;
                }
                if mv.is_none() {
                    mv = e.mv;
                }
                target = i;
                break;
            }
            let age_diff = ((age + 64 - e_age) & 63) as i32;
            let val = e.depth - 8 * age_diff;
            if val < target_val {
                target_val = val;
                target = i;
            }
        }
        let data = pack(mv, score, eval, depth, bound, age);
        let slot = &bucket.slots[target];
        slot.data.store(data, Relaxed);
        slot.key.store(hash ^ data, Relaxed);
    }

    /// Fuellstand in Promille (Stichprobe der ersten 1000 Eintraege).
    pub fn hashfull(&self) -> usize {
        let age = self.age.load(Relaxed) & 63;
        let mut used = 0;
        let mut seen = 0;
        'outer: for b in &self.buckets {
            for slot in &b.slots {
                if seen >= 1000 {
                    break 'outer;
                }
                seen += 1;
                let d = slot.data.load(Relaxed);
                if d != 0 && unpack(d).1 == age {
                    used += 1;
                }
            }
        }
        if seen == 0 {
            0
        } else {
            used * 1000 / seen
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_roundtrip() {
        for &(score, eval, depth, bound) in
            &[(0, 0, 0, BOUND_UPPER), (-31000, 1234, 99, BOUND_LOWER), (31127, -32001, 255, BOUND_EXACT)]
        {
            let m = Move(0xBEEF);
            let (e, age) = unpack(pack(m, score, eval, depth, bound, 37));
            assert_eq!((e.mv, e.score, e.eval, e.depth, e.bound, age), (m, score, eval, depth, bound, 37));
        }
    }

    #[test]
    fn store_probe() {
        let tt = TT::new(1);
        tt.new_search();
        let h = 0x1234_5678_9ABC_DEF0;
        assert!(tt.probe(h).is_none());
        tt.store(h, Move(77), 50, 40, 7, BOUND_EXACT);
        let e = tt.probe(h).unwrap();
        assert_eq!((e.mv, e.score, e.eval, e.depth, e.bound), (Move(77), 50, 40, 7, BOUND_EXACT));
        // Zug bleibt erhalten, wenn ohne Zug neu gespeichert wird.
        tt.store(h, Move::NONE, 10, 40, 8, BOUND_UPPER);
        assert_eq!(tt.probe(h).unwrap().mv, Move(77));
        tt.clear();
        assert!(tt.probe(h).is_none());
    }
}
