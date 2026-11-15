"""UCI-Protokoll-Konformitaetstests fuer Turmfalke.

Jeder Check laeuft mit einem frischen Engine-Prozess und einem Watchdog; haengt ein
Check, wird die Engine gekillt und der Check als FAIL gewertet. Exit-Code 0 nur,
wenn alle Checks bestanden sind.

Beispiel:
    python uci_test.py
    python uci_test.py --engine mock_engine.py --only ponder -v
"""
from __future__ import annotations

import argparse
import os
import random
import re
import sys
import threading
import time
import traceback
from dataclasses import dataclass
from typing import Callable, Optional

import chess

import uci_driver as ud
from uci_driver import EngineCrashed, EngineError, EngineStartError, EngineTimeout


class Fail(Exception):
    pass


# --------------------------------------------------------------------------
# Testpositionen (werden beim Start mit python-chess geprueft)
# --------------------------------------------------------------------------
KIWIPETE = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1"
CASTLE_BLACK = "r3k2r/pppppppp/8/8/8/8/PPPPPPPP/R3K2R b KQkq - 0 1"
EP_WHITE_SAN = ["e4", "d5", "e5", "f5"]           # weiss kann exf6 e.p.
EP_BLACK_SAN = ["a3", "e5", "a4", "e4", "d4"]     # schwarz kann exd3 e.p.
PROMO_WHITE = "1r2k3/P7/8/8/8/8/8/4K3 w - - 0 1"   # a8=Q/R/B/N und axb8=...
PROMO_BLACK = "4k3/8/8/8/8/8/p7/1R2K3 b - - 0 1"
CHECKMATED = "rnb1kbnr/pppp1ppp/8/4p3/6Pq/5P2/PPPPP2P/RNBQKBNR w KQkq - 1 3"  # Narrenmatt
STALEMATED = "7k/5Q2/6K1/8/8/8/8/8 b - - 0 1"
ONLY_MOVE_A = "5R2/8/k7/8/8/8/6PP/r6K w - - 0 1"   # nur Rf1 (Schach blocken)
ONLY_MOVE_B = "k7/8/8/8/8/P7/5q2/7K w - - 0 1"     # nur a3-a4
MATE_IN_1 = "6k1/5ppp/8/8/8/8/5PPP/R5K1 w - - 0 1"  # Grundreihe Ra8#
MIDDLEGAME = "r1bqk2r/pp2bppp/2n1pn2/2pp4/3P4/2PBPN2/PP1N1PPP/R1BQK2R w KQkq - 0 7"
ENDGAME = "8/5pk1/6p1/3R4/7P/6P1/r4PK1/8 w - - 0 40"
PERFT_POSITIONS = [
    ("Startstellung", chess.STARTING_FEN, 4),
    ("Kiwipete", KIWIPETE, 3),
    ("Position 3", "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1", 5),
    ("Position 4", "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1", 3),
    ("Position 5", "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8", 3),
]


def board_from_san(sans) -> chess.Board:
    b = chess.Board()
    for s in sans:
        b.push_san(s)
    return b


def random_game_board(plies: int = 80, seed: int = 20261114) -> chess.Board:
    """Deterministische Zufallspartie mit genau ``plies`` Halbzuegen (Spiel nicht beendet)."""
    for s in range(seed, seed + 1000):
        rng = random.Random(s)
        b = chess.Board()
        ok = True
        for _ in range(plies):
            moves = sorted(b.legal_moves, key=lambda m: m.uci())
            if not moves:
                ok = False
                break
            b.push(rng.choice(moves))
            if b.is_game_over(claim_draw=False):
                ok = False
                break
        if ok and any(b.legal_moves):
            return b
    raise RuntimeError("keine passende Zufallspartie gefunden")


def validate_fixtures() -> list:
    """Prueft alle festen Testpositionen mit python-chess; Rueckgabe: Fehlerliste."""
    errs = []

    def need(cond, msg):
        if not cond:
            errs.append(msg)

    for fen in (KIWIPETE, CASTLE_BLACK, PROMO_WHITE, PROMO_BLACK, CHECKMATED, STALEMATED,
                ONLY_MOVE_A, ONLY_MOVE_B, MATE_IN_1, MIDDLEGAME, ENDGAME):
        b = chess.Board(fen)
        need(b.is_valid(), f"ungueltige FEN: {fen}")
    need(any(chess.Board(KIWIPETE).generate_castling_moves()), "Kiwipete: keine Rochade")
    need(any(chess.Board(CASTLE_BLACK).generate_castling_moves()), "CASTLE_BLACK: keine Rochade")
    for sans in (EP_WHITE_SAN, EP_BLACK_SAN):
        b = board_from_san(sans)
        need(any(b.is_en_passant(m) for m in b.legal_moves), f"kein e.p. nach {sans}")
    for fen in (PROMO_WHITE, PROMO_BLACK):
        need(any(m.promotion for m in chess.Board(fen).legal_moves), f"keine Umwandlung: {fen}")
    need(chess.Board(CHECKMATED).is_checkmate(), "CHECKMATED ist nicht matt")
    need(chess.Board(STALEMATED).is_stalemate(), "STALEMATED ist nicht patt")
    for fen in (ONLY_MOVE_A, ONLY_MOVE_B):
        need(chess.Board(fen).legal_moves.count() == 1, f"nicht genau ein legaler Zug: {fen}")
    need(len(mating_moves(chess.Board(MATE_IN_1))) >= 1, "MATE_IN_1 hat kein Matt in 1")
    return errs


def mating_moves(board: chess.Board) -> set:
    res = set()
    for m in board.legal_moves:
        board.push(m)
        if board.is_checkmate():
            res.add(m.uci())
        board.pop()
    return res


# --------------------------------------------------------------------------
# Check-Infrastruktur
# --------------------------------------------------------------------------
@dataclass
class Check:
    name: str
    fn: Callable
    setup: bool
    timeout: float
    extra: bool


CHECKS: list = []


def check(name: str, setup: bool = True, timeout: float = 30.0, extra: bool = False):
    def deco(fn):
        CHECKS.append(Check(name, fn, setup, timeout, extra))
        return fn
    return deco


class Ctx:
    def __init__(self, args):
        self.args = args
        self.eng: Optional[ud.Engine] = None
        self.uci_info: Optional[ud.UciInfo] = None

    def start(self) -> ud.Engine:
        self.eng = ud.Engine(self.args.engine).start()
        return self.eng

    def setup(self) -> None:
        self.start()
        self.uci_info = self.eng.uci(timeout=10.0)
        self.eng.set_options(ud.parse_opts(self.args.opts))
        self.eng.isready(timeout=15.0)

    def close(self) -> None:
        if self.eng is not None:
            self.eng.quit(timeout=1.5)


def has_bestmove(lines) -> Optional[str]:
    for l in lines:
        if l.split()[:1] == ["bestmove"]:
            return l
    return None


def search(eng: ud.Engine, board: chess.Board, go: str, timeout: float) -> ud.SearchResult:
    eng.position(board)
    return eng.search(go, timeout)


def assert_legal(board: chess.Board, res: ud.SearchResult) -> chess.Move:
    mv = ud.legal_move(board, res.bestmove)
    if mv is None:
        raise Fail(f"illegaler bestmove '{res.bestmove}' in {board.fen()}")
    if res.ponder is not None:
        b = board.copy()
        b.push(mv)
        if ud.legal_move(b, res.ponder) is None:
            raise Fail(f"illegaler ponder-Zug '{res.ponder}' nach {res.bestmove}")
    return mv


def assert_infos_ok(board: chess.Board, infos: list) -> tuple:
    """Alle info-Zeilen: Scores parsbar, pv legal. Rueckgabe (#infos, #pvs)."""
    n_pv = 0
    for info in infos:
        if info["errors"]:
            raise Fail(f"info-Zeile fehlerhaft: {'; '.join(info['errors'])}")
        if "pv" in info:
            ok, idx, mv = ud.validate_pv(board, info["pv"])
            if not ok:
                raise Fail(f"illegale pv (Zug {idx + 1} '{mv}'): {' '.join(info['pv'])}")
            n_pv += 1
    return len(infos), n_pv


# --------------------------------------------------------------------------
# Checks: Handshake / Optionen
# --------------------------------------------------------------------------
EXPECTED_OPTIONS = {
    "Hash": {"type": "spin", "default": 256, "min": 1, "max": 65536},
    "Threads": {"type": "spin", "default": 8, "min": 1, "max": 256},
    "Move Overhead": {"type": "spin", "default": 30, "min": 0, "max": 5000},
    "Clear Hash": {"type": "button"},
    "Ponder": {"type": "check", "default": "false"},
}


def find_option(info: ud.UciInfo, name: str):
    for k, v in info.options.items():
        if k.lower() == name.lower():
            return v
    return None


@check("uci -> uciok innerhalb 3 s", setup=False)
def c_uciok(ctx):
    eng = ctx.start()
    try:
        info = eng.uci(timeout=3.0)
    except EngineTimeout:
        raise Fail("kein uciok innerhalb 3 s")
    return f"{info.elapsed_ms:.0f} ms"


@check("uci: 'id name' und 'id author' vorhanden")
def c_id(ctx):
    info = ctx.uci_info
    if not info.name:
        raise Fail("keine 'id name'-Zeile vor uciok")
    if not info.author:
        raise Fail(f"keine 'id author'-Zeile (name='{info.name}')")
    return f"'{info.name}' von '{info.author}'"


@check("uci: alle 5 Optionen mit richtigem Typ")
def c_option_types(ctx):
    problems = []
    for name, exp in EXPECTED_OPTIONS.items():
        opt = find_option(ctx.uci_info, name)
        if opt is None:
            problems.append(f"{name} fehlt")
        elif opt["type"] != exp["type"]:
            problems.append(f"{name}: Typ '{opt['type']}' statt '{exp['type']}'")
    if problems:
        raise Fail("; ".join(problems))
    return ", ".join(EXPECTED_OPTIONS)


@check("uci: Options-Defaults und Grenzen laut Vertrag")
def c_option_values(ctx):
    problems = []
    for name, exp in EXPECTED_OPTIONS.items():
        opt = find_option(ctx.uci_info, name)
        if opt is None:
            problems.append(f"{name} fehlt")
            continue
        for key in ("default", "min", "max"):
            if key not in exp:
                continue
            got = opt.get(key)
            want = exp[key]
            if isinstance(want, int):
                ok = got is not None and re.fullmatch(r"-?\d+", got) and int(got) == want
            else:
                ok = (got or "").lower() == want
            if not ok:
                problems.append(f"{name}.{key}={got!r} (erwartet {want})")
    if problems:
        raise Fail("; ".join(problems))
    return "ok"


@check("isready -> readyok")
def c_isready(ctx):
    ms = ctx.eng.isready(timeout=2.0)
    return f"{ms:.0f} ms"


@check("setoption Hash 16 / Threads 2 (+ Move Overhead, Clear Hash) -> isready")
def c_setoption(ctx):
    eng = ctx.eng
    eng.setoption("Hash", 16)
    eng.setoption("Threads", 2)
    ms = eng.isready(timeout=5.0)
    eng.setoption("Move Overhead", 50)
    eng.setoption("Clear Hash")
    eng.isready(timeout=5.0)
    board = chess.Board()
    res = search(eng, board, "go movetime 200", timeout=5.0)
    assert_legal(board, res)
    return f"readyok nach {ms:.0f} ms, Suche danach ok ({res.bestmove})"


# --------------------------------------------------------------------------
# Checks: Suche
# --------------------------------------------------------------------------
@check("go movetime 500 nach e2e4 e7e5 g1f3: legal, <= 900 ms")
def c_movetime(ctx):
    board = board_from_san(["e4", "e5", "Nf3"])
    res = search(ctx.eng, board, "go movetime 500", timeout=6.0)
    assert_legal(board, res)
    if res.elapsed_ms > 900:
        raise Fail(f"bestmove nach {res.elapsed_ms:.0f} ms (Limit 900 ms)")
    return f"{res.bestmove} nach {res.elapsed_ms:.0f} ms"


@check("info: pv legal, score/Felder parsbar", timeout=40.0)
def c_info_pv(ctx):
    total_i = total_pv = 0
    boards = [board_from_san(["e4", "e5", "Nf3"]), chess.Board(KIWIPETE),
              chess.Board(MIDDLEGAME), chess.Board(ENDGAME), board_from_san(EP_WHITE_SAN)]
    for board in boards:
        res = search(ctx.eng, board, "go movetime 600", timeout=6.0)
        assert_legal(board, res)
        n_i, n_pv = assert_infos_ok(board, res.infos)
        if not any("score" in i for i in res.infos):
            raise Fail(f"keine info-Zeile mit score ({board.fen()})")
        if n_pv == 0:
            raise Fail(f"keine info-Zeile mit pv ({board.fen()})")
        total_i += n_i
        total_pv += n_pv
    return f"{total_i} info-Zeilen, {total_pv} pv geprueft"


@check("go infinite: kein bestmove vor stop, isready waehrend Suche, stop -> bestmove <= 300 ms")
def c_infinite(ctx):
    eng = ctx.eng
    board = chess.Board(MIDDLEGAME)
    eng.position(board)
    eng.send("go infinite")
    lines = eng.drain(1.0)
    if has_bestmove(lines):
        raise Fail(f"bestmove ohne stop: {has_bestmove(lines)}")
    try:
        ready_ms = eng.isready(timeout=1.0)
    except EngineTimeout:
        raise Fail("isready waehrend go infinite nicht innerhalb 1 s beantwortet")
    if has_bestmove(eng.last_lines):
        raise Fail("bestmove ohne stop (waehrend isready)")
    if ready_ms > 500:
        raise Fail(f"readyok erst nach {ready_ms:.0f} ms waehrend der Suche (Limit 500 ms)")
    lines = eng.drain(0.2)
    if has_bestmove(lines):
        raise Fail("bestmove ohne stop (nach isready)")
    t0 = eng.send("stop")
    try:
        line, lines = eng.wait_for("bestmove", 3.0)
    except EngineTimeout:
        raise Fail("kein bestmove innerhalb 3 s nach stop")
    res = eng.make_result(line, lines, t0)
    assert_legal(board, res)
    if res.elapsed_ms > 300:
        raise Fail(f"bestmove {res.elapsed_ms:.0f} ms nach stop (Limit 300 ms)")
    return f"readyok {ready_ms:.0f} ms, bestmove {res.bestmove} {res.elapsed_ms:.0f} ms nach stop"


@check("go depth 8: legaler bestmove", timeout=70.0)
def c_depth(ctx):
    board = chess.Board(MIDDLEGAME)
    try:
        res = search(ctx.eng, board, "go depth 8", timeout=60.0)
    except EngineTimeout:
        raise Fail("go depth 8 nicht innerhalb 60 s fertig")
    assert_legal(board, res)
    assert_infos_ok(board, res.infos)
    note = "" if res.max_depth >= 8 else f" (Hinweis: max. gemeldete Tiefe {res.max_depth})"
    return f"{res.bestmove}, {res.elapsed_ms:.0f} ms{note}"


@check("go nodes 20000: legaler bestmove", timeout=40.0)
def c_nodes(ctx):
    board = chess.Board(KIWIPETE)
    try:
        res = search(ctx.eng, board, "go nodes 20000", timeout=30.0)
    except EngineTimeout:
        raise Fail("go nodes 20000 nicht innerhalb 30 s fertig")
    assert_legal(board, res)
    nodes = max((i.get("nodes", 0) for i in res.infos), default=0)
    return f"{res.bestmove}, {res.elapsed_ms:.0f} ms, max. nodes {nodes}"


def _positions_check(ctx, boards, go="go movetime 300"):
    moves = []
    for board in boards:
        res = search(ctx.eng, board, go, timeout=5.0)
        assert_legal(board, res)
        assert_infos_ok(board, res.infos)
        moves.append(res.bestmove)
    return ", ".join(moves)


@check("FEN mit Rochaderechten (weiss/schwarz)")
def c_fen_castling(ctx):
    return _positions_check(ctx, [chess.Board(KIWIPETE), chess.Board(CASTLE_BLACK)])


@check("FEN/Zugliste mit en-passant-Feld (weiss/schwarz)")
def c_fen_ep(ctx):
    boards = []
    for sans in (EP_WHITE_SAN, EP_BLACK_SAN):
        b = board_from_san(sans)
        boards.append(chess.Board(b.fen()))  # als FEN mit e.p.-Feld
        boards.append(b)                      # als startpos + moves
    return _positions_check(ctx, boards)


@check("FEN mit moeglichen Umwandlungen (weiss/schwarz)")
def c_fen_promo(ctx):
    return _positions_check(ctx, [chess.Board(PROMO_WHITE), chess.Board(PROMO_BLACK)])


@check("startpos + 80 Halbzuege (Zufallspartie, fester Seed)")
def c_long_moves(ctx):
    board = random_game_board()
    return f"{len(board.move_stack)} Halbzuege, " + _positions_check(ctx, [board])


@check("mattgesetzte Stellung -> bestmove 0000")
def c_checkmated(ctx):
    board = chess.Board(CHECKMATED)
    res = search(ctx.eng, board, "go movetime 500", timeout=3.0)
    if res.bestmove != "0000":
        raise Fail(f"bestmove '{res.bestmove}' statt 0000")
    return f"0000 nach {res.elapsed_ms:.0f} ms"


@check("Pattstellung -> bestmove 0000")
def c_stalemated(ctx):
    board = chess.Board(STALEMATED)
    res = search(ctx.eng, board, "go movetime 500", timeout=3.0)
    if res.bestmove != "0000":
        raise Fail(f"bestmove '{res.bestmove}' statt 0000")
    return f"0000 nach {res.elapsed_ms:.0f} ms"


@check("genau ein legaler Zug -> dieser Zug")
def c_only_move(ctx):
    out = []
    for fen in (ONLY_MOVE_A, ONLY_MOVE_B):
        board = chess.Board(fen)
        only = next(iter(board.legal_moves)).uci()
        res = search(ctx.eng, board, "go movetime 500", timeout=3.0)
        if res.bestmove != only:
            raise Fail(f"bestmove '{res.bestmove}' statt {only} ({fen})")
        out.append(f"{only} {res.elapsed_ms:.0f} ms")
    return ", ".join(out)


@check("Matt in 1 -> richtiger Zug und 'score mate 1'")
def c_mate1(ctx):
    board = chess.Board(MATE_IN_1)
    want = mating_moves(board)
    res = search(ctx.eng, board, "go movetime 1000", timeout=4.0)
    if res.bestmove not in want:
        raise Fail(f"bestmove '{res.bestmove}' statt {sorted(want)}")
    if res.last_score != ("mate", 1):
        raise Fail(f"letzter Score {res.last_score} statt ('mate', 1)")
    return f"{res.bestmove}, score mate 1"


# --------------------------------------------------------------------------
# Checks: Zeitsteuerung / Ponder
# --------------------------------------------------------------------------
@check("go wtime 2000 btime 2000 -> < 600 ms")
def c_clock_2000(ctx):
    board = chess.Board(MIDDLEGAME)
    res = search(ctx.eng, board, "go wtime 2000 btime 2000", timeout=5.0)
    assert_legal(board, res)
    if res.elapsed_ms >= 600:
        raise Fail(f"{res.elapsed_ms:.0f} ms (Limit < 600 ms)")
    return f"{res.bestmove} nach {res.elapsed_ms:.0f} ms"


@check("go wtime 50 btime 50 -> legal, <= 250 ms")
def c_clock_50(ctx):
    out = []
    for board in (chess.Board(MIDDLEGAME), chess.Board(KIWIPETE)):
        res = search(ctx.eng, board, "go wtime 50 btime 50", timeout=3.0)
        assert_legal(board, res)
        if res.elapsed_ms > 250:
            raise Fail(f"{res.elapsed_ms:.0f} ms (Limit 250 ms)")
        out.append(f"{res.bestmove} {res.elapsed_ms:.0f} ms")
    return ", ".join(out)


@check("go ponder: kein bestmove vor ponderhit, danach bestmove <= 2.5 s")
def c_ponderhit(ctx):
    eng = ctx.eng
    eng.setoption("Ponder", "true")
    eng.isready(5.0)
    board = board_from_san(["e4", "e5", "Nf3", "Nc6"])
    eng.position(board)
    eng.send("go ponder wtime 5000 btime 5000")
    lines = eng.drain(0.5)
    if has_bestmove(lines):
        raise Fail(f"bestmove waehrend ponder: {has_bestmove(lines)}")
    t0 = eng.send("ponderhit")
    try:
        line, lines = eng.wait_for("bestmove", 5.0)
    except EngineTimeout:
        raise Fail("kein bestmove innerhalb 5 s nach ponderhit")
    res = eng.make_result(line, lines, t0)
    assert_legal(board, res)
    if res.elapsed_ms > 2500:
        raise Fail(f"bestmove {res.elapsed_ms:.0f} ms nach ponderhit (Limit 2500 ms)")
    return f"{res.bestmove} {res.elapsed_ms:.0f} ms nach ponderhit"


@check("go ponder + stop (Ponder-Miss) -> bestmove <= 300 ms")
def c_ponder_stop(ctx):
    eng = ctx.eng
    eng.setoption("Ponder", "true")
    eng.isready(5.0)
    board = board_from_san(["d4", "Nf6", "c4"])
    eng.position(board)
    eng.send("go ponder wtime 5000 btime 5000 winc 100 binc 100")
    lines = eng.drain(0.4)
    if has_bestmove(lines):
        raise Fail(f"bestmove waehrend ponder: {has_bestmove(lines)}")
    t0 = eng.send("stop")
    try:
        line, lines = eng.wait_for("bestmove", 3.0)
    except EngineTimeout:
        raise Fail("kein bestmove innerhalb 3 s nach stop")
    res = eng.make_result(line, lines, t0)
    assert_legal(board, res)
    if res.elapsed_ms > 300:
        raise Fail(f"bestmove {res.elapsed_ms:.0f} ms nach stop (Limit 300 ms)")
    # danach muss die Engine normal weiterarbeiten
    board2 = board_from_san(["d4", "Nf6", "c4", "e6"])
    res2 = search(eng, board2, "go movetime 200", timeout=3.0)
    assert_legal(board2, res2)
    return f"{res.bestmove} {res.elapsed_ms:.0f} ms nach stop, Folge-Suche ok"


@check("ucinewgame zwischen Suchen")
def c_ucinewgame(ctx):
    eng = ctx.eng
    seq = [(board_from_san(["e4"]), "go movetime 200"),
           (board_from_san(["d4", "d5", "c4"]), "go movetime 200"),
           (chess.Board(KIWIPETE), "go depth 5"),
           (chess.Board(), "go wtime 3000 btime 3000 winc 50 binc 50")]
    moves = []
    for board, go in seq:
        eng.send("ucinewgame")
        eng.isready(5.0)
        res = search(eng, board, go, timeout=10.0)
        assert_legal(board, res)
        moves.append(res.bestmove)
    return ", ".join(moves)


@check("quit -> Prozessende innerhalb 2 s")
def c_quit(ctx):
    eng = ctx.eng
    t0 = eng.send("quit")
    try:
        eng.proc.wait(timeout=2.0)
    except Exception:
        raise Fail("Prozess laeuft 2 s nach quit noch")
    return f"beendet nach {(time.perf_counter() - t0) * 1000:.0f} ms, Exit-Code {eng.proc.returncode}"


@check("quit waehrend go infinite -> Prozessende innerhalb 2 s")
def c_quit_search(ctx):
    eng = ctx.eng
    eng.position(chess.Board(MIDDLEGAME))
    eng.send("go infinite")
    eng.drain(0.3)
    t0 = eng.send("quit")
    try:
        eng.proc.wait(timeout=2.0)
    except Exception:
        raise Fail("Prozess laeuft 2 s nach quit noch")
    return f"beendet nach {(time.perf_counter() - t0) * 1000:.0f} ms"


# --------------------------------------------------------------------------
# Zusatz-Checks (nicht-standard: d, perft, bench) - abschaltbar mit --no-extras
# --------------------------------------------------------------------------
def fen_matches(engine_fen: str, board: chess.Board) -> bool:
    """Vergleich tolerant beim e.p.-Feld (immer / nur wenn legal / nur wenn pseudo-legal)."""
    got = engine_fen.split()
    if len(got) != 6:
        return False
    for ep in ("legal", "fen", "xfen"):
        want = board.fen(en_passant=ep).split()
        if got == want:
            return True
    return False


def _d_fen(ctx, board):
    eng = ctx.eng
    eng.position(board)
    eng.send("d")
    try:
        line, _ = eng.wait_for("Fen:", 3.0)
    except EngineTimeout:
        raise Fail("keine 'Fen:'-Zeile nach 'd'")
    fen = line.split(None, 1)[1].strip() if len(line.split()) > 1 else ""
    if not fen_matches(fen, board):
        raise Fail(f"Fen falsch: '{fen}' statt '{board.fen(en_passant='fen')}'")
    return fen


@check("d: Fen nach startpos + 80 Halbzuegen und nach e.p.-Zugfolge", extra=True)
def c_d_fen(ctx):
    _d_fen(ctx, random_game_board())
    _d_fen(ctx, board_from_san(EP_WHITE_SAN))
    _d_fen(ctx, board_from_san(EP_BLACK_SAN))
    _d_fen(ctx, chess.Board(KIWIPETE))
    return "4 Stellungen korrekt"


def py_perft(board: chess.Board, depth: int) -> int:
    if depth <= 1:
        return board.legal_moves.count() if depth == 1 else 1
    total = 0
    for m in board.legal_moves:
        board.push(m)
        total += py_perft(board, depth - 1)
        board.pop()
    return total


def py_divide(board: chess.Board, depth: int) -> dict:
    res = {}
    for m in board.legal_moves:
        board.push(m)
        res[m.uci()] = py_perft(board, depth - 1)
        board.pop()
    return res


DIVIDE_RE = re.compile(r"^([a-h][1-8][a-h][1-8][qrbn]?)\s*:\s*(\d+)\s*$")


def _make_perft_check(label, fen, depth):
    @check(f"perft {depth} {label}", extra=True, timeout=120.0)
    def _c(ctx):
        board = chess.Board(fen)
        want = py_divide(board, depth)
        eng = ctx.eng
        eng.position(board)
        eng.send(f"perft {depth}")
        try:
            line, lines = eng.wait_for("Nodes searched:", 90.0)
        except EngineTimeout:
            raise Fail("keine 'Nodes searched:'-Zeile innerhalb 90 s")
        try:
            total = int(line.split(":", 1)[1].strip())
        except (IndexError, ValueError):
            raise Fail(f"unparsbar: '{line}'")
        got = {}
        for l in lines:
            m = DIVIDE_RE.match(l.strip())
            if m:
                got[m.group(1)] = int(m.group(2))
        want_total = sum(want.values())
        if total != want_total:
            diffs = [f"{k}: {got.get(k)} statt {v}" for k, v in sorted(want.items())
                     if got.get(k) != v]
            extra = sorted(set(got) - set(want))
            if extra:
                diffs.append("zusaetzliche Zuege: " + " ".join(extra))
            raise Fail(f"{total} statt {want_total}; " + "; ".join(diffs[:8]))
        if got and got != want:
            raise Fail("Summe stimmt, aber Aufteilung pro Zug weicht ab")
        return f"{total} Knoten" + ("" if got else " (keine Aufteilung pro Zug gefunden)")
    return _c


for _label, _fen, _depth in PERFT_POSITIONS:
    _make_perft_check(_label, _fen, _depth)


BENCH_RE = re.compile(r"^\s*(\d+)\s+nodes\s+(\d+)\s+nps\s*$")


@check("bench: Schlusszeile '<n> nodes <nps> nps'", extra=True, timeout=200.0)
def c_bench(ctx):
    eng = ctx.eng
    eng.send("bench")
    deadline = time.perf_counter() + 180.0
    while True:
        remaining = deadline - time.perf_counter()
        if remaining <= 0:
            raise Fail("keine bench-Schlusszeile innerhalb 180 s")
        lines = eng.drain(min(0.5, remaining))
        for l in lines:
            m = BENCH_RE.match(l)
            if m:
                return f"{int(m.group(1))} nodes, {int(m.group(2))} nps"


# --------------------------------------------------------------------------
# Runner
# --------------------------------------------------------------------------
def run_check(chk: Check, args) -> tuple:
    ctx = Ctx(args)
    result = {}

    def target():
        try:
            if chk.setup:
                ctx.setup()
            detail = chk.fn(ctx)
            result["ok"], result["detail"] = True, detail or ""
        except Fail as exc:
            result["ok"], result["detail"] = False, str(exc)
        except EngineStartError as exc:
            result["ok"], result["detail"] = False, str(exc)
        except EngineTimeout as exc:
            result["ok"], result["detail"] = False, f"Timeout: {exc}"
        except EngineCrashed as exc:
            result["ok"], result["detail"] = False, f"Absturz: {exc}"
        except Exception as exc:  # Harness-Fehler sichtbar machen
            result["ok"] = False
            result["detail"] = f"Harness-Fehler {type(exc).__name__}: {exc}"
            if args.verbose:
                traceback.print_exc()

    t0 = time.perf_counter()
    th = threading.Thread(target=target, daemon=True)
    th.start()
    th.join(chk.timeout)
    if th.is_alive():
        if ctx.eng is not None:
            ctx.eng.kill()
        th.join(5.0)
        result = {"ok": False, "detail": f"Check-Timeout nach {chk.timeout:.0f} s (haengt)"}
    tail = ctx.eng.tail(30 if args.verbose else 8) if (ctx.eng and not result.get("ok")) else []
    ctx.close()
    return result.get("ok", False), result.get("detail", "kein Ergebnis"), time.perf_counter() - t0, tail


def main(argv=None) -> int:
    ud.setup_console()
    sys.setswitchinterval(0.001)  # genauere Zeitstempel im Reader-Thread
    p = argparse.ArgumentParser(description="UCI-Konformitaetstests fuer Turmfalke")
    ud.add_engine_argument(p)
    p.add_argument("--opts", default="", help="Optionen nach uci, z.B. 'Threads=1,Hash=64' "
                   "(Default: Engine-Defaults)")
    p.add_argument("--only", default="", help="nur Checks, deren Name diesen Text enthaelt")
    p.add_argument("--no-extras", action="store_true", help="d/perft/bench-Checks auslassen")
    p.add_argument("--list", action="store_true", help="Checks nur auflisten")
    p.add_argument("-v", "--verbose", action="store_true", help="mehr Engine-Log bei FAIL")
    args = p.parse_args(argv)

    checks = [c for c in CHECKS if args.only.lower() in c.name.lower()
              and not (args.no_extras and c.extra)]
    if args.list:
        for c in checks:
            print(("[extra] " if c.extra else "        ") + c.name)
        return 0

    if not os.path.isfile(args.engine):
        print(f"Engine nicht gefunden: {args.engine}")
        return 1
    errs = validate_fixtures()
    if errs:
        print("Harness-Fehler in Testpositionen:")
        for e in errs:
            print("  " + e)
        return 2

    print(f"Engine: {args.engine}")
    print(f"{len(checks)} Checks\n")
    passed = 0
    failed = []
    for c in checks:
        ok, detail, dur, tail = run_check(c, args)
        tag = "PASS" if ok else "FAIL"
        print(f"[{tag}] {c.name}  ({detail}) [{dur:.1f} s]")
        if not ok:
            failed.append(c.name)
            for l in tail:
                print("        | " + l)
        passed += ok
    print()
    print(f"Ergebnis: {passed}/{len(checks)} bestanden"
          + (f", {len(failed)} fehlgeschlagen" if failed else ""))
    return 0 if passed == len(checks) and checks else 1


if __name__ == "__main__":
    sys.exit(main())
