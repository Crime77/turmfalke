#![allow(dead_code)] // einige Hilfsfunktionen werden nur in Tests benutzt

//! Turmfalke - eine selbstgeschriebene UCI-Schach-Engine.
//!
//! Start ohne Argumente: UCI-Modus (Befehle ueber stdin).
//! Start mit Argumenten: Befehl ausfuehren und beenden, z. B. `turmfalke bench`.

mod attacks;
mod eval;
mod movegen;
mod perft;
mod position;
mod search;
mod smp;
mod time;
mod tt;
mod types;
mod uci;
mod zobrist;

fn main() {
    attacks::init();
    eval::init();
    std::sync::LazyLock::force(&zobrist::ZOBRIST);
    time::now_ms();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        let mut u = uci::Uci::new();
        u.handle(&args.join(" "));
        u.handle("quit");
        return;
    }
    search::send(&format!("{} - UCI-Schach-Engine (Befehl 'uci' zum Starten)", uci::NAME));
    uci::run();
}
