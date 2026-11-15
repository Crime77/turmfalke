"""Engine-gegen-Engine-Match mit python-chess als strengem Schiedsrichter.

* Eroeffnungen: eingebaute Liste (4-10 Halbzuege), beim Start validiert; jede
  Eroeffnung wird zweimal mit vertauschten Farben gespielt.
* Uhren: echte Buchfuehrung (Wandzeit pro Zug ab 'go' bis 'bestmove', abziehen,
  Inkrement addieren). Uhr < 0 => Zeitverlust UND FEHLER.
* Illegaler Zug, Absturz oder keine Antwort innerhalb Restzeit + 2 s => FEHLER
  (FEN, Zugliste und letzte Engine-Ausgabe werden protokolliert).
* Partieende nur nach Regeln (python-chess, claim_draw=True): Matt, Patt,
  ungenuegendes Material, dreifache Wiederholung, 50-Zuege-Regel. Keine Adjudication.
* Ausgabe: Fortschrittszeile pro Partie, am Ende +/=/- aus Sicht von A, Score,
  Elo-Differenz mit 95-%-Fehlerbalken, Fehler nach Typ; alle Partien als PGN.
Exit-Code 1, wenn irgendein FEHLER auftrat.
"""
from __future__ import annotations

import argparse
import datetime
import math
import os
import queue
import sys
import threading
import time
from dataclasses import dataclass, field
from typing import Optional

import chess
import chess.pgn

import uci_driver as ud
from uci_driver import EngineCrashed, EngineError, EngineTimeout

# --------------------------------------------------------------------------
# Eroeffnungen (SAN, werden beim Start mit python-chess validiert)
# --------------------------------------------------------------------------
OPENINGS_SAN = [
    "e4 e5 Nf3 Nc6 Bb5 a6",                      # Spanisch
    "e4 e5 Nf3 Nc6 Bc4 Bc5",                     # Italienisch
    "e4 e5 Nf3 Nc6 d4 exd4",                     # Schottisch
    "e4 e5 Nf3 Nf6 Nxe5 d6",                     # Russisch
    "e4 e5 f4 exf4 Nf3 g5",                      # Koenigsgambit
    "e4 e5 Nc3 Nf6 f4 d5",                       # Wiener Partie
    "e4 e5 Nf3 Nc6 Nc3 Nf6",                     # Vierspringer
    "e4 e5 Nf3 d6 d4 Nf6",                       # Philidor
    "e4 c5 Nf3 d6 d4 cxd4 Nxd4 Nf6 Nc3 a6",      # Najdorf
    "e4 c5 Nf3 Nc6 d4 cxd4 Nxd4 g6",             # Beschleunigter Drache
    "e4 c5 Nc3 Nc6 g3 g6 Bg2 Bg7",               # Geschlossener Sizilianer
    "e4 c5 c3 Nf6 e5 Nd5",                       # Alapin
    "e4 c5 Nf3 e6 d4 cxd4 Nxd4 Nc6",             # Taimanov
    "e4 c5 Nf3 d6 Bb5+ Bd7",                     # Moskauer Variante
    "e4 e6 d4 d5 Nc3 Bb4",                       # Franzoesisch Winawer
    "e4 e6 d4 d5 e5 c5 c3 Nc6",                  # Franzoesisch Vorstoss
    "e4 c6 d4 d5 e5 Bf5",                        # Caro-Kann Vorstoss
    "e4 c6 d4 d5 Nc3 dxe4 Nxe4 Bf5",             # Caro-Kann klassisch
    "e4 d5 exd5 Qxd5 Nc3 Qa5",                   # Skandinavisch
    "e4 d6 d4 Nf6 Nc3 g6",                       # Pirc
    "e4 g6 d4 Bg7 Nc3 d6",                       # Moderne Verteidigung
    "e4 Nf6 e5 Nd5 d4 d6",                       # Aljechin
    "d4 d5 c4 e6 Nc3 Nf6 Bg5 Be7",               # Abgelehntes Damengambit
    "d4 d5 c4 dxc4 Nf3 Nf6 e3 e6",               # Angenommenes Damengambit
    "d4 d5 c4 c6 Nf3 Nf6 Nc3 dxc4",              # Slawisch
    "d4 d5 c4 e6 Nc3 c5 cxd5 exd5",              # Tarrasch
    "d4 Nf6 c4 g6 Nc3 Bg7 e4 d6",                # Koenigsindisch
    "d4 Nf6 c4 e6 Nc3 Bb4",                      # Nimzoindisch
    "d4 Nf6 c4 e6 Nf3 b6",                       # Damenindisch
    "d4 Nf6 c4 g6 Nc3 d5 cxd5 Nxd5",             # Gruenfeld
    "d4 Nf6 c4 c5 d5 b5",                        # Wolga/Benko
    "d4 Nf6 c4 c5 d5 e6 Nc3 exd5 cxd5 d6",       # Moderne Benoni
    "d4 f5 g3 Nf6 Bg2 g6",                       # Hollaendisch Leningrad
    "d4 d5 Bf4 Nf6 e3 c5",                       # Londoner System
    "d4 Nf6 Bg5 Ne4",                            # Trompowsky
    "c4 e5 Nc3 Nf6 g3 d5 cxd5 Nxd5",             # Englisch (umgekehrter Sizilianer)
    "c4 c5 Nc3 Nc6 g3 g6 Bg2 Bg7",               # Englisch symmetrisch
    "Nf3 d5 g3 Nf6 Bg2 e6 O-O Be7",              # Reti / KIA
    "Nf3 Nf6 c4 b6 g3 Bb7 Bg2 e6",               # Reti
    "f4 d5 Nf3 Nf6 e3 g6",                       # Bird
    "b3 e5 Bb2 Nc6 e3 d5",                       # Larsen
    "g3 d5 Bg2 e5 d3 Nf6",                       # Koenigsfianchetto
]

FAIL_LABELS = {
    "time_loss": "Zeitverlust",
    "illegal_move": "illegaler Zug",
    "crash": "Absturz",
    "no_answer": "keine Antwort",
    "start": "Start/Handshake",
}
WARN_LABELS = {"illegal_pv": "illegale PV", "illegal_ponder": "illegaler Ponder-Zug",
               "bad_info": "unparsbare info"}
TERMINATION_DE = {
    chess.Termination.CHECKMATE: "Matt",
    chess.Termination.STALEMATE: "Patt",
    chess.Termination.INSUFFICIENT_MATERIAL: "ungenuegendes Material",
    chess.Termination.SEVENTYFIVE_MOVES: "75-Zuege-Regel",
    chess.Termination.FIVEFOLD_REPETITION: "fuenffache Wiederholung",
    chess.Termination.FIFTY_MOVES: "50-Zuege-Regel",
    chess.Termination.THREEFOLD_REPETITION: "dreifache Wiederholung",
}


def validate_openings():
    good, bad = [], []
    for line in OPENINGS_SAN:
        b = chess.Board()
        try:
            moves = []
            for san in line.split():
                mv = b.parse_san(san)
                moves.append(mv)
                b.push(mv)
            if not 4 <= len(moves) <= 10:
                raise ValueError(f"{len(moves)} Halbzuege (erlaubt 4-10)")
            if b.is_game_over(claim_draw=True):
                raise ValueError("Stellung ist bereits beendet")
            good.append((line, moves, chess.Board().variation_san(moves)))
        except ValueError as exc:
            bad.append((line, str(exc)))
    return good, bad


def parse_tc(spec: str):
    try:
        if "+" in spec:
            base, inc = spec.split("+", 1)
        else:
            base, inc = spec, "0"
        base, inc = int(float(base)), int(float(inc))
        if base <= 0 or inc < 0:
            raise ValueError
        return base, inc
    except ValueError:
        raise argparse.ArgumentTypeError(f"ungueltige Zeitkontrolle '{spec}' (Format base_ms+inc_ms)")


# --------------------------------------------------------------------------
# Statistik
# --------------------------------------------------------------------------
def elo_from_score(p: float) -> float:
    if p <= 0.0:
        return -math.inf
    if p >= 1.0:
        return math.inf
    return -400.0 * math.log10(1.0 / p - 1.0)


def elo_stats(w: int, d: int, l: int):
    """(score, elo, margin95, los) wie bei cutechess-cli."""
    n = w + d + l
    if n == 0:
        return None
    p = (w + 0.5 * d) / n
    var = (w * (1 - p) ** 2 + d * (0.5 - p) ** 2 + l * (0 - p) ** 2) / n
    sd = math.sqrt(var / n)
    lo, hi = p - 1.959964 * sd, p + 1.959964 * sd
    elo = elo_from_score(p)
    if lo <= 0 or hi >= 1 or sd == 0:
        margin = math.inf if sd > 0 or not 0 < p < 1 else 0.0
    else:
        margin = (elo_from_score(hi) - elo_from_score(lo)) / 2
    los = 0.5 * (1 + math.erf((w - l) / math.sqrt(2 * (w + l)))) if w + l > 0 else 0.5
    return p, elo, margin, los


def fmt_num(x: float, plus: bool = False) -> str:
    if math.isinf(x):
        return ("+" if x > 0 else "-") + "inf" if plus or x < 0 else "inf"
    return f"{x:+.1f}" if plus else f"{x:.1f}"


# --------------------------------------------------------------------------
# Spieler / Partie
# --------------------------------------------------------------------------
class Player:
    def __init__(self, label: str, path: str, opts: list):
        self.label = label
        self.path = path
        self.opts = opts
        self.eng: Optional[ud.Engine] = None
        self.dirty = False
        self.name = f"{os.path.basename(path)} ({label})"

    def ensure(self) -> ud.Engine:
        if self.eng is not None and self.eng.is_alive() and not self.dirty:
            return self.eng
        if self.eng is not None:
            self.eng.quit(timeout=1.0)
        self.eng = None
        eng = ud.Engine(self.path).start()
        self.eng = eng
        info = eng.uci(timeout=10.0)
        eng.set_options(self.opts)
        eng.isready(timeout=15.0)
        self.dirty = False
        if info.name:
            self.name = f"{info.name} ({self.label})"
        return eng

    def kill(self):
        if self.eng is not None:
            self.eng.kill()

    def close(self):
        if self.eng is not None:
            self.eng.quit(timeout=2.0)
            self.eng = None


@dataclass
class Failure:
    kind: str
    game: int
    engine: str
    fen: str
    moves: list
    detail: str
    tail: list


@dataclass
class GameResult:
    index: int
    opening_index: int
    a_white: bool
    white: str
    black: str
    moves: list
    comments: list
    result: str = "*"
    reason: str = ""
    termination: str = "normal"
    failure: Optional[Failure] = None
    warnings: list = field(default_factory=list)  # (kind, engine_label, text)
    clocks: tuple = (0.0, 0.0)

    @property
    def a_points(self) -> Optional[float]:
        if self.result == "1-0":
            return 1.0 if self.a_white else 0.0
        if self.result == "0-1":
            return 0.0 if self.a_white else 1.0
        if self.result == "1/2-1/2":
            return 0.5
        return None


def loss_for(color: bool) -> str:
    return "0-1" if color == chess.WHITE else "1-0"


def play_game(index: int, opening_index: int, opening_moves: list, a_white: bool,
              pa: Player, pb: Player, base: int, inc: int) -> GameResult:
    board = chess.Board()
    for mv in opening_moves:
        board.push(mv)
    players = {chess.WHITE: pa if a_white else pb, chess.BLACK: pb if a_white else pa}
    gr = GameResult(index, opening_index, a_white, players[chess.WHITE].name,
                    players[chess.BLACK].name, list(opening_moves), ["Buch"] * len(opening_moves))

    def fail(kind, color, detail, eng=None):
        pl = players[color]
        tail = eng.tail(25) if eng is not None else []
        gr.failure = Failure(kind, index, pl.label, board.fen(), [m.uci() for m in board.move_stack],
                             detail, tail)
        gr.result = loss_for(color)
        gr.reason = f"{FAIL_LABELS[kind]} {pl.label}"
        gr.termination = {"time_loss": "time forfeit", "illegal_move": "rules infraction"}.get(
            kind, "abandoned")
        if kind in ("crash", "no_answer", "start"):
            pl.dirty = True

    # Engines bereitstellen
    for color in (chess.WHITE, chess.BLACK):
        pl = players[color]
        try:
            eng = pl.ensure()
            eng.send("ucinewgame")
            eng.isready(timeout=10.0)
        except EngineError as exc:
            fail("start", color, f"{type(exc).__name__}: {exc}", pl.eng)
            return gr
    gr.white, gr.black = players[chess.WHITE].name, players[chess.BLACK].name

    clocks = {chess.WHITE: float(base), chess.BLACK: float(base)}
    while True:
        outcome = board.outcome(claim_draw=True)
        if outcome is not None:
            gr.result = outcome.result()
            gr.reason = TERMINATION_DE.get(outcome.termination, outcome.termination.name)
            break
        turn = board.turn
        pl = players[turn]
        eng = pl.eng
        remaining = clocks[turn]
        try:
            eng.position(board)
            t0 = eng.send(f"go wtime {max(0, int(clocks[chess.WHITE]))} "
                          f"btime {max(0, int(clocks[chess.BLACK]))} winc {inc} binc {inc}")
            line, lines = eng.wait_for("bestmove", (remaining + 2000.0) / 1000.0)
        except EngineTimeout:
            fail("no_answer", turn, f"kein bestmove innerhalb Restzeit {remaining:.0f} ms + 2 s", eng)
            break
        except EngineCrashed as exc:
            fail("crash", turn, str(exc), eng)
            break
        res = eng.make_result(line, lines, t0)
        clocks[turn] -= res.elapsed_ms
        if clocks[turn] < 0:
            fail("time_loss", turn, f"Uhr {clocks[turn]:.0f} ms (Zug dauerte {res.elapsed_ms:.0f} ms "
                 f"bei {remaining:.0f} ms Restzeit, bestmove '{res.bestmove}')", eng)
            if board.has_insufficient_material(not turn):
                gr.result = "1/2-1/2"
                gr.reason += " (Gegner kann nicht mattsetzen: remis)"
            break
        mv = ud.legal_move(board, res.bestmove)
        if mv is None:
            fail("illegal_move", turn, f"bestmove '{res.bestmove}' illegal ('{line}')", eng)
            break
        # Warnungen (kein FEHLER): illegale PV / Ponder-Zuege, unparsbare info-Zeilen
        for info in res.infos:
            if info["errors"]:
                gr.warnings.append(("bad_info", pl.label, "; ".join(info["errors"])))
                break
        for info in res.infos:
            if info.get("pv"):
                ok, idx, bad = ud.validate_pv(board, info["pv"])
                if not ok:
                    gr.warnings.append(("illegal_pv", pl.label,
                                        f"Ply {len(board.move_stack) + 1}: pv-Zug {idx + 1} '{bad}'"))
                    break
        if res.ponder is not None:
            b2 = board.copy(stack=False)
            b2.push(mv)
            if ud.legal_move(b2, res.ponder) is None:
                gr.warnings.append(("illegal_ponder", pl.label, f"ponder '{res.ponder}'"))
        clocks[turn] += inc
        last = res.last_info_with_pv or (res.infos[-1] if res.infos else {})
        score = res.last_score
        gr.comments.append(f"{ud.format_score(score)}/{last.get('depth', 0)} {res.elapsed_ms / 1000:.3f}s")
        board.push(mv)
        gr.moves.append(mv)
    gr.clocks = (clocks[chess.WHITE], clocks[chess.BLACK])
    return gr


# --------------------------------------------------------------------------
# PGN
# --------------------------------------------------------------------------
def pgn_safe(text: str) -> str:
    return text.replace("{", "(").replace("}", ")")


def to_pgn(gr: GameResult, opening_line: str, base: int, inc: int, date: str) -> chess.pgn.Game:
    game = chess.pgn.Game()
    h = game.headers
    h["Event"] = "Turmfalke Selfplay"
    h["Site"] = "lokal"
    h["Date"] = date
    h["Round"] = str(gr.index + 1)
    h["White"] = gr.white
    h["Black"] = gr.black
    h["Result"] = gr.result
    h["TimeControl"] = f"{base / 1000:g}+{inc / 1000:g}"
    h["Termination"] = gr.termination
    h["Opening"] = opening_line
    h["PlyCount"] = str(len(gr.moves))
    if gr.failure:
        h["Failure"] = f"{gr.failure.kind} ({gr.failure.engine})"
    node = game
    for mv, cm in zip(gr.moves, gr.comments):
        node = node.add_variation(mv)
        node.comment = pgn_safe(cm)
    note = gr.reason
    if gr.failure:
        f = gr.failure
        note = (f"FEHLER {FAIL_LABELS[f.kind]} durch {f.engine}: {f.detail}. FEN: {f.fen}. "
                f"Letzte Ausgabe: " + " | ".join(l.strip()[:120] for l in f.tail[-6:]))
    for kind, eng, text in gr.warnings[:5]:
        note += f" | Warnung {WARN_LABELS[kind]} ({eng}): {text}"
    note = pgn_safe(note)
    if node is game:
        game.comment = note
    else:
        node.comment = (node.comment + " " + note).strip()
    return game


# --------------------------------------------------------------------------
# Hauptprogramm
# --------------------------------------------------------------------------
def main(argv=None) -> int:
    ud.setup_console()
    sys.setswitchinterval(0.001)  # zuegigere Thread-Wechsel -> genauere Zeitstempel
    p = argparse.ArgumentParser(description="Selfplay-Match mit python-chess als Schiedsrichter")
    ud.add_engine_argument(p, "--engine-a", "engine_a", "Engine A (Default: Turmfalke-Release)")
    ud.add_engine_argument(p, "--engine-b", "engine_b", "Engine B (Default: Turmfalke-Release)")
    p.add_argument("--opts-a", default="Threads=1,Hash=64", help="setoption fuer A (Default %(default)s)")
    p.add_argument("--opts-b", default="Threads=1,Hash=64", help="setoption fuer B (Default %(default)s)")
    p.add_argument("--games", type=int, default=100, help="Anzahl Partien (Default 100)")
    p.add_argument("--concurrency", type=int, default=4, help="parallele Partien (Default 4)")
    p.add_argument("--tc", type=parse_tc, default=(10000, 100), metavar="BASE_MS+INC_MS",
                   help="Zeitkontrolle in ms (Default 10000+100)")
    p.add_argument("--pgn", default=os.path.join(ud.SCRIPTS_DIR, "selfplay.pgn"),
                   help="PGN-Ausgabedatei (Default: selfplay.pgn im Skriptordner)")
    p.add_argument("--no-fail-details", action="store_true", help="Fehlerdetails nicht ausgeben")
    args = p.parse_args(argv)
    base, inc = args.tc

    for path in (args.engine_a, args.engine_b):
        if not os.path.isfile(path):
            print(f"Engine nicht gefunden: {path}")
            return 1
    openings, bad = validate_openings()
    for line, err in bad:
        print(f"Eroeffnung verworfen: '{line}': {err}")
    if not openings:
        print("Keine gueltigen Eroeffnungen.")
        return 1
    if args.games <= 0:
        print("--games muss > 0 sein")
        return 1

    schedule = queue.Queue()
    for i in range(args.games):
        schedule.put((i, (i // 2) % len(openings), i % 2 == 0))
    opts_a, opts_b = ud.parse_opts(args.opts_a), ud.parse_opts(args.opts_b)
    k = max(1, min(args.concurrency, args.games))
    print(f"Match: A={os.path.basename(args.engine_a)} [{args.opts_a}]  vs  B={os.path.basename(args.engine_b)} [{args.opts_b}]")
    print(f"{args.games} Partien, TC {base}+{inc} ms, {k} parallel, {len(openings)} Eroeffnungen\n")

    lock = threading.Lock()
    results: list = []
    stop = threading.Event()
    all_players: list = []
    stats = {"w": 0, "d": 0, "l": 0}

    def worker():
        pa = Player("A", args.engine_a, opts_a)
        pb = Player("B", args.engine_b, opts_b)
        with lock:
            all_players.extend([pa, pb])
        try:
            while not stop.is_set():
                try:
                    i, oi, a_white = schedule.get_nowait()
                except queue.Empty:
                    break
                try:
                    gr = play_game(i, oi, openings[oi][1], a_white, pa, pb, base, inc)
                except Exception as exc:  # Harness-Fehler: Partie markieren, weitermachen
                    gr = GameResult(i, oi, a_white, pa.name, pb.name, [], [])
                    gr.reason = f"Harness-Fehler: {type(exc).__name__}: {exc}"
                    gr.result = "*"
                    pa.dirty = pb.dirty = True
                if stop.is_set():
                    break
                with lock:
                    results.append(gr)
                    pts = gr.a_points
                    if pts == 1.0:
                        stats["w"] += 1
                    elif pts == 0.5:
                        stats["d"] += 1
                    elif pts == 0.0:
                        stats["l"] += 1
                    nmoves = (len(gr.moves) + 1) // 2
                    fail_txt = f"  FEHLER: {FAIL_LABELS[gr.failure.kind]} ({gr.failure.engine})" \
                        if gr.failure else ""
                    warn_txt = f"  [{len(gr.warnings)} Warnung(en)]" if gr.warnings else ""
                    print(f"Partie {len(results):>3}/{args.games} (#{i + 1}, Eroeffnung {oi + 1}): "
                          f"{gr.white} - {gr.black}  {gr.result}  {gr.reason}, {nmoves} Zuege"
                          f"  | A: +{stats['w']} ={stats['d']} -{stats['l']}{fail_txt}{warn_txt}")
        finally:
            pa.close()
            pb.close()

    threads = [threading.Thread(target=worker, daemon=True) for _ in range(k)]
    t_start = time.perf_counter()
    for t in threads:
        t.start()
    interrupted = False
    try:
        while any(t.is_alive() for t in threads):
            for t in threads:
                t.join(timeout=0.2)
    except KeyboardInterrupt:
        interrupted = True
        stop.set()
        print("\nAbbruch durch Benutzer - Engines werden beendet ...")
        with lock:
            for pl in all_players:
                pl.kill()
        for t in threads:
            t.join(timeout=5.0)
    duration = time.perf_counter() - t_start

    results.sort(key=lambda g: g.index)
    # PGN schreiben
    date = datetime.date.today().strftime("%Y.%m.%d")
    try:
        with open(args.pgn, "w", encoding="utf-8") as f:
            for gr in results:
                print(to_pgn(gr, openings[gr.opening_index][2], base, inc, date),
                      file=f, end="\n\n")
        pgn_msg = f"PGN: {args.pgn} ({len(results)} Partien)"
    except OSError as exc:
        pgn_msg = f"PGN konnte nicht geschrieben werden: {exc}"

    # Zusammenfassung
    w, d, l = stats["w"], stats["d"], stats["l"]
    n = w + d + l
    failures = [g.failure for g in results if g.failure]
    print()
    print(f"=== Ergebnis nach {len(results)} Partien ({duration:.0f} s)"
          + (" - ABGEBROCHEN" if interrupted else "") + " ===")
    name_a = next((g.white if g.a_white else g.black for g in results), "A")
    print(f"Aus Sicht von A ({name_a}): +{w} ={d} -{l}")
    st = elo_stats(w, d, l)
    if st:
        score, elo, margin, los = st
        print(f"Score: {100 * score:.1f} %   Elo-Differenz: {fmt_num(elo, True)} +/- {fmt_num(margin)} "
              f"(95 %)   LOS: {100 * los:.1f} %   Remisquote: {100 * d / n:.1f} %")
    undecided = len(results) - n
    if undecided:
        print(f"Ohne Wertung (Harness-Fehler): {undecided}")
    if failures:
        parts = []
        for kind, label in FAIL_LABELS.items():
            fs = [f for f in failures if f.kind == kind]
            if fs:
                na = sum(1 for f in fs if f.engine == "A")
                parts.append(f"{label}: {len(fs)} (A {na}, B {len(fs) - na})")
        print("FEHLER: " + "; ".join(parts))
    else:
        print("Fehler: keine (kein Zeitverlust, kein illegaler Zug, kein Absturz, keine Haenger)")
    warns = [w_ for g in results for w_ in g.warnings]
    if warns:
        parts = []
        for kind, label in WARN_LABELS.items():
            ws = [x for x in warns if x[0] == kind]
            if ws:
                parts.append(f"{label}: {len(ws)}")
        print("Warnungen: " + "; ".join(parts) + " (Details im PGN)")
    print(pgn_msg)

    if failures and not args.no_fail_details:
        print("\n--- Fehlerdetails ---")
        for f in failures:
            print(f"Partie #{f.game + 1}: {FAIL_LABELS[f.kind]} durch Engine {f.engine}: {f.detail}")
            print(f"  FEN:   {f.fen}")
            print(f"  Zuege: {' '.join(f.moves)}")
            print("  Letzte Engine-Ausgabe:")
            for line in f.tail[-12:]:
                print("    | " + line)
    harness_errors = [g for g in results if g.result == "*"]
    for g in harness_errors:
        print(f"Partie #{g.index + 1}: {g.reason}")
    ok = not failures and not harness_errors and not interrupted and len(results) == args.games
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
