# Turmfalke – UCI-Schach-Engine

Komplett selbst geschriebene Schach-Engine in Rust (nur Standardbibliothek).
Sie spricht das **UCI-Protokoll** und läuft im Terminal oder in jeder Schach-Oberfläche.

## Bauen

Benötigt wird nur [Rust](https://rustup.rs) (keine weiteren Bibliotheken).

```bash
git clone https://github.com/Crime77/turmfalke.git
cd turmfalke
cargo build --release
```

Das Programm liegt danach unter `target/release/turmfalke.exe`.
Es ist für die CPU optimiert, auf der es gebaut wurde (`target-cpu=native`).

## Spielen

### In einer Schach-Oberfläche (empfohlen)
Kostenlose Oberflächen: **Cute Chess**, **Arena**, **BanksiaGUI**.
Dort „Engine hinzufügen“ wählen, `turmfalke.exe` auswählen, Protokoll **UCI**.

### Direkt im Terminal
```
turmfalke.exe
uci
position startpos moves e2e4
go movetime 5000
```
Die Engine antwortet mit `bestmove …`. Danach den eigenen Zug anhängen,
z. B. `position startpos moves e2e4 e7e5 g1f3`, und wieder `go …` senden.

## UCI-Optionen

| Option | Standard | Bedeutung |
|---|---|---|
| `Hash` | 256 | Größe der Hash-Tabelle in MB |
| `Threads` | 8 | Anzahl Such-Threads (Lazy SMP), höchstens so viele wie die CPU Threads hat |
| `Move Overhead` | 30 | Sicherheitsreserve pro Zug in ms (gegen Zeitüberschreitung) |
| `Clear Hash` | – | Hash-Tabelle leeren |
| `Ponder` | aus | Denken in der Zeit des Gegners |

## Zusatzbefehle

| Befehl | Wirkung |
|---|---|
| `d` | Brett und FEN anzeigen |
| `eval` | statische Bewertung der aktuellen Stellung |
| `perft N` | alle Zugfolgen bis Tiefe N zählen (Prüfung des Zuggenerators) |
| `bench [Tiefe]` | feste Testsuche, gibt Knoten und Knoten/s aus |

`turmfalke.exe bench` führt den Befehl direkt aus und beendet sich.

## Hinweise für Partien und Turniere

- **CPU nicht überbuchen:** `Threads` × gleichzeitig laufende Partien sollte
  nicht größer sein als die Zahl der CPU-Threads. Bei starker Überlast pausiert
  Windows die Engine zu lange und Blitzpartien können auf Zeit verloren gehen.
  Wer trotzdem überbucht, sollte `Move Overhead` erhöhen (z. B. 100).
- Bei sehr knapper Restzeit (Suche unter 200 ms) rechnet nur ein Thread. Das
  kostet kaum Stärke und schützt vor Zeitüberschreitung.
- Ungültige Eingaben (kaputte FEN, illegaler Zug in `position … moves`) werden
  mit `info string …` gemeldet. Ein folgendes `go` antwortet mit `bestmove 0000`
  statt einen Zug in einer falschen Stellung zu spielen.
- FEN mit Rochaderechten im X-FEN-Format (`HAha`) wird verstanden.

## Technik

- Bitboards mit Magic Bitboards, Zobrist-Hashing
- Suche: iterative Vertiefung, Aspiration Windows, Principal Variation Search,
  Ruhesuche mit SEE, Null-Move-Pruning, Reverse Futility Pruning, Razoring,
  Futility Pruning, Late Move Reductions/Pruning, Schachverlängerung,
  Mate-Distance-Pruning, Internal Iterative Reduction
- Zugsortierung: Hash-Zug, gute Schlagzüge (SEE + MVV-LVA), Killer-Züge,
  Countermoves, History-Heuristik
- Lock-freie Hash-Tabelle, die sich alle Threads teilen (Lazy SMP)
- Handgeschriebene Bewertung mit Mittel-/Endspiel-Übergang

## Tests

```bash
cargo test --release
```
Prüft u. a. Perft auf sechs Standardstellungen, Hash-Konsistenz, SEE,
Remis-Erkennung, Zeitplanung, Matt-Erkennung und zufällige Partien.

Die Python-Skripte in `scripts/` (benötigen `pip install chess`) prüfen das
UCI-Protokoll, Taktikaufgaben und spielen Partien Engine gegen Engine mit
python-chess als unabhängigem Schiedsrichter.

Letzter Stand (Ryzen 7 7800X3D):
- Rust-Tests 33/33, UCI-Test 33/33, Taktik 32/32, 400 go/stop-Zyklen fehlerfrei
- 100 Selbstspielpartien (10 s + 0,1 s): kein Zeitverlust, kein illegaler Zug,
  kein Absturz (`scripts/selfplay_v2.pgn`)
- 8 Threads gegen 1 Thread, 30 Partien: +14 =16 −0, etwa +176 Elo (±82)
  (`scripts/smp_8v1.pgn`)

## Ehrliche Einordnung

Keine Engine ist mathematisch unschlagbar, denn Schach ist nicht gelöst.
Turmfalke ist für menschliche Gegner sehr stark. Gegen Spitzen-Engines wie
Stockfish verliert sie aber. Es gibt kein Eröffnungsbuch und keine
Endspiel-Datenbanken.

## Lizenz

Turmfalke steht unter der **GNU General Public License v3.0 oder neuer**
(siehe [LICENSE](LICENSE)). Du darfst den Code frei nutzen, verändern und
weitergeben. Veränderte Versionen, die du weitergibst, müssen ebenfalls unter
der GPL stehen und ihren Quellcode offenlegen.
