"""Taktik-Testsuite fuer Turmfalke.

Jede Stellung wird VOR dem Engine-Lauf mit python-chess verifiziert:
  * board.is_valid(), alle akzeptierten Zuege legal
  * Matt-Aufgaben: exakte Brute-Force-Suche, dass jeder akzeptierte Zug Matt in <= N
    erzwingt. Die Menge der akzeptierten Zuege wird auf ALLE Zuege erweitert, die
    Matt in <= N erzwingen (sonst wuerde eine gleichwertige Loesung als Fehler gelten).
  * Material-Taktiken: Alpha-Beta (Material-Eval + Quiescence) mit fester Tiefe muss
    bestaetigen, dass der Schluesselzug klar Material gewinnt und klar besser ist als
    jeder andere Zug.
Nicht verifizierbare Eintraege werden verworfen und ausgegeben.

Danach: Engine mit 'go movetime <ms>' auf jeder Stellung; Matt-Aufgaben muessen
zusaetzlich mit 'score mate N' (0 < N <= erwartet) enden.
"""
from __future__ import annotations

import argparse
import sys
import time

import chess

import uci_driver as ud
from uci_driver import EngineCrashed, EngineError, EngineStartError, EngineTimeout

# --------------------------------------------------------------------------
# Testdaten: (id, Thema, FEN, akzeptierte Zuege (UCI), Matt in N oder None)
# --------------------------------------------------------------------------
POSITIONS = [
    # ---- Matt in 1 -------------------------------------------------------
    ("M01", "Grundreihenmatt (Schwarz am Zug)",
     "1r4k1/5ppp/8/8/8/8/5PPP/6K1 b - - 0 1", ["b8b1"], 1),
    ("M02", "Ersticktes Matt in 1",
     "6rk/6pp/8/6N1/8/8/8/6K1 w - - 0 1", ["g5f7"], 1),
    ("M03", "Unterverwandlung c8=N matt",
     "rn6/kpP5/p7/8/8/8/8/7K w - - 0 1", ["c7c8n"], 1),
    ("M04", "Schaefermatt Qxf7#",
     "r1bqkb1r/pppp1ppp/2n2n2/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR w KQkq - 4 4", ["h5f7"], 1),
    ("M05", "Narrenmatt (Schwarz am Zug)",
     "rnbqkbnr/pppp1ppp/8/4p3/6P1/5P2/PPPPP2P/RNBQKBNR b KQkq - 0 2", ["d8h4"], 1),
    ("M06", "Arabisches Matt",
     "7k/1R6/5N2/8/8/8/6PP/6K1 w - - 0 1", ["b7h7"], 1),
    # ---- Matt in 2 -------------------------------------------------------
    ("M07", "Grundreihe: Turmverdopplung",
     "1r4k1/5ppp/8/8/8/8/3R1PPP/3R2K1 w - - 0 1", ["d2d8"], 2),
    ("M08", "Grundreihe: Turmverdopplung (Schwarz am Zug)",
     "3r2k1/3r1ppp/8/8/8/8/5PPP/1R4K1 b - - 0 1", ["d7d1"], 2),
    ("M09", "Ersticktes Matt nach Damenopfer",
     "5r1k/6pp/7N/3Q4/8/8/5PPP/6K1 w - - 0 1", ["d5g8"], 2),
    ("M10", "Legal-Matt",
     "rn1qkbnr/ppp2p1p/3p2p1/4N3/2B1P3/2N5/PPPP1PPP/R1BbK2R w KQkq - 0 6", ["c4f7"], 2),
    # ---- Matt in 3 -------------------------------------------------------
    ("M11", "Philidors Vermaechtnis (Doppelschach, Damenopfer, ersticktes Matt)",
     "5rk1/5Npp/8/8/2Q5/8/5PPP/6K1 w - - 0 1", ["f7h6"], 3),
    # ---- stille Schluesselzuege (per Brute-Force gefunden, eindeutige Loesung) ----
    ("M12", "Matt in 2, stiller Springerzug",
     "6k1/1Q6/Kp6/2N1p3/8/8/8/8 w - - 0 1", ["c5e6"], 2),
    ("M13", "Matt in 2, stiller Laeuferzug",
     "8/5R2/k7/2K1B3/8/5p2/8/8 w - - 0 1", ["e5b8"], 2),
    ("M14", "Matt in 2, stiller Turmzug",
     "6R1/2K5/k7/8/3p4/8/3B4/8 w - - 0 1", ["g8g5"], 2),
    ("M15", "Matt in 2, stiller Koenigszug (Schwarz am Zug)",
     "8/1P6/3P4/8/2n5/3q4/4k2K/8 b - - 0 1", ["e2f2"], 2),
    ("M16", "Matt in 3, K+T gegen K, stiller Koenigszug",
     "6k1/8/8/4R2K/8/8/8/8 w - - 0 1", ["h5g6"], 3),
    ("M17", "Matt in 3, stiller Koenigszug (D+L gegen T)",
     "4k3/1QK5/8/8/8/B1p5/8/3r4 w - - 0 1", ["c7c6"], 3),
    ("M18", "Matt in 3, stiller Koenigszug (T+S gegen S)",
     "k6N/5R2/8/1K6/6n1/4p3/8/8 w - - 0 1", ["b5b6"], 3),
    # ---- Material-Taktiken ----------------------------------------------
    ("T01", "Springergabel Koenig+Turm",
     "2r3k1/5ppp/8/3N4/8/8/5PPP/6K1 w - - 0 1", ["d5e7"], None),
    ("T02", "Springergabel Koenig+Dame",
     "3q3k/6pp/8/6N1/8/8/6PP/6K1 w - - 0 1", ["g5f7"], None),
    ("T03", "Bauerngabel Springer+Turm",
     "4k3/8/2n1r3/8/3P4/7P/5RPK/8 w - - 0 1", ["d4d5"], None),
    ("T04", "Fesselung: Laeufer fesselt Dame an Koenig",
     "4k3/p2q3p/8/8/8/2N5/PP5P/4KB2 w - - 0 1", ["f1b5"], None),
    ("T05", "Fesselung: Turm fesselt Dame an Koenig",
     "4k3/ppp5/8/4q3/8/8/2BK1PPP/R7 w - - 0 1", ["a1e1"], None),
    ("T06", "Spiess: Turm Koenig->Dame",
     "8/8/2k4q/8/8/8/8/R5K1 w - - 0 1", ["a1a6"], None),
    ("T07", "Spiess: Laeufer Koenig->Dame",
     "4q3/p7/2k5/8/8/8/7P/3B2K1 w - - 0 1", ["d1a4"], None),
    ("T08", "Damengabel Koenig+Turm",
     "r5k1/6pp/8/8/8/8/6PP/3Q2K1 w - - 0 1", ["d1d5"], None),
    ("T09", "Laeufergabel Koenig+Turm",
     "6k1/6pp/8/8/8/5B2/r4PPP/6K1 w - - 0 1", ["f3d5"], None),
    ("T10", "Abzugsschach: Springer greift Dame an",
     "3q3k/5p1p/6p1/8/3N4/8/1B3PPP/6K1 w - - 0 1", ["d4c6", "d4e6"], None),
    ("T11", "Springergabel Koenig+Turm (Schwarz am Zug)",
     "6k1/5ppp/8/8/3n4/8/5PPP/2R3K1 b - - 0 1", ["d4e2"], None),
    ("T12", "Spiess: Laeufer Koenig->Dame (Schwarz am Zug)",
     "3b2k1/7p/8/8/8/2K5/P7/4Q3 b - - 0 1", ["d8a5"], None),
    ("T13", "Fesselung: Turm fesselt Dame (Schwarz am Zug)",
     "r7/2bk1ppp/8/8/4Q3/8/PPP5/4K3 b - - 0 1", ["a8e8"], None),
    ("T14", "Springergabel Koenig+Dame (Schwarz am Zug)",
     "6k1/6pp/8/4n3/8/8/3Q1P1P/6K1 b - - 0 1", ["e5f3"], None),
]

PIECE_VALUES = {chess.PAWN: 100, chess.KNIGHT: 320, chess.BISHOP: 330,
                chess.ROOK: 500, chess.QUEEN: 900, chess.KING: 0}
MATE_SCORE = 100000
MIN_GAIN = 200     # Schluesselzug muss >= 2 Bauern netto gewinnen
MIN_MARGIN = 150   # ... und >= 1.5 Bauern besser sein als jeder andere Zug


class TooExpensive(Exception):
    pass


def tkey(board: chess.Board):
    try:
        return board._transposition_key()
    except AttributeError:  # Fallback fuer andere python-chess-Versionen
        return board.epd()


# --------------------------------------------------------------------------
# Exakte Mattsuche
# --------------------------------------------------------------------------
class MateSolver:
    """Exakte Brute-Force-Suche: kann die Seite am Zug in <= n Zuegen mattsetzen?"""

    def __init__(self, node_limit: int = 3_000_000):
        self.tt = {}
        self.nodes = 0
        self.node_limit = node_limit

    def _count(self):
        self.nodes += 1
        if self.nodes > self.node_limit:
            raise TooExpensive(f"Knotenlimit {self.node_limit} ueberschritten")

    def _ordered(self, board: chess.Board):
        checks, captures, quiet = [], [], []
        for m in board.legal_moves:
            if board.gives_check(m):
                checks.append(m)
            elif board.is_capture(m) or m.promotion:
                captures.append(m)
            else:
                quiet.append(m)
        return checks + captures + quiet

    def mate_in(self, board: chess.Board, n: int) -> bool:
        if n <= 0:
            return False
        key = (tkey(board), n)
        hit = self.tt.get(key)
        if hit is not None:
            return hit
        self._count()
        result = False
        if n == 1:
            for m in board.legal_moves:
                if board.gives_check(m):
                    board.push(m)
                    mate = board.is_checkmate()
                    board.pop()
                    if mate:
                        result = True
                        break
        else:
            for m in self._ordered(board):
                if self.forces_mate(board, m, n):
                    result = True
                    break
        self.tt[key] = result
        return result

    def forces_mate(self, board: chess.Board, move: chess.Move, n: int) -> bool:
        """Erzwingt ``move`` (Seite am Zug) Matt in <= n eigenen Zuegen?"""
        board.push(move)
        try:
            if board.is_checkmate():
                return True
            if n <= 1:
                return False
            replies = list(board.legal_moves)
            if not replies:  # Patt
                return False
            if board.is_insufficient_material():
                return False
            for r in replies:
                board.push(r)
                ok = self.mate_in(board, n - 1)
                board.pop()
                if not ok:
                    return False
            return True
        finally:
            board.pop()

    def shortest_mate(self, board: chess.Board, max_n: int):
        for k in range(1, max_n + 1):
            if self.mate_in(board, k):
                return k
        return None

    def all_mating_moves(self, board: chess.Board, n: int) -> set:
        return {m.uci() for m in board.legal_moves if self.forces_mate(board, m, n)}


# --------------------------------------------------------------------------
# Material-Alpha-Beta
# --------------------------------------------------------------------------
class MaterialSearch:
    def __init__(self, node_limit: int = 4_000_000):
        self.nodes = 0
        self.node_limit = node_limit

    @staticmethod
    def material(board: chess.Board) -> int:
        s = 0
        for pt, v in PIECE_VALUES.items():
            s += v * (len(board.pieces(pt, chess.WHITE)) - len(board.pieces(pt, chess.BLACK)))
        return s if board.turn == chess.WHITE else -s

    @staticmethod
    def _victim_value(board, m):
        if board.is_en_passant(m):
            return 100
        p = board.piece_at(m.to_square)
        return PIECE_VALUES[p.piece_type] if p else 0

    def _order(self, board, moves):
        def key(m):
            v = self._victim_value(board, m) + (800 if m.promotion == chess.QUEEN else 0)
            a = board.piece_type_at(m.from_square) or 0
            return -(v * 10 - a)
        return sorted(moves, key=key)

    def _count(self):
        self.nodes += 1
        if self.nodes > self.node_limit:
            raise TooExpensive(f"Knotenlimit {self.node_limit} ueberschritten")

    def qsearch(self, board, alpha, beta, ply):
        self._count()
        stand = self.material(board)
        if stand >= beta:
            return stand
        if stand > alpha:
            alpha = stand
        moves = [m for m in board.legal_moves
                 if board.is_capture(m) or m.promotion == chess.QUEEN]
        if not moves and not any(board.legal_moves):
            return -MATE_SCORE + ply if board.is_check() else 0
        best = stand
        for m in self._order(board, moves):
            board.push(m)
            score = -self.qsearch(board, -beta, -alpha, ply + 1)
            board.pop()
            if score > best:
                best = score
            if score > alpha:
                alpha = score
            if alpha >= beta:
                break
        return best

    def negamax(self, board, depth, alpha, beta, ply):
        self._count()
        moves = list(board.legal_moves)
        if not moves:
            return -MATE_SCORE + ply if board.is_check() else 0
        if board.is_insufficient_material():
            return 0
        if depth <= 0:
            return self.qsearch(board, alpha, beta, ply)
        best = -MATE_SCORE * 2
        for m in self._order(board, moves):
            board.push(m)
            score = -self.negamax(board, depth - 1, -beta, -alpha, ply + 1)
            board.pop()
            if score > best:
                best = score
            if score > alpha:
                alpha = score
            if alpha >= beta:
                break
        return best

    def root_scores(self, board, depth) -> dict:
        res = {}
        for m in board.legal_moves:
            board.push(m)
            res[m.uci()] = -self.negamax(board, depth - 1, -MATE_SCORE * 2, MATE_SCORE * 2, 1)
            board.pop()
        return res


# --------------------------------------------------------------------------
# Verifikation
# --------------------------------------------------------------------------
def verify_entry(entry, depth: int):
    """Rueckgabe (ok, accepted_set, info_text)."""
    pid, theme, fen, best, mate = entry
    try:
        board = chess.Board(fen)
    except ValueError as exc:
        return False, None, f"FEN unparsbar: {exc}"
    if not board.is_valid():
        return False, None, f"ungueltige Stellung ({board.status()!r})"
    if board.is_game_over():
        return False, None, "Stellung ist bereits beendet"
    for uci in best:
        if ud.legal_move(board, uci) is None:
            return False, None, f"akzeptierter Zug {uci} ist illegal"
    if mate:
        solver = MateSolver()
        try:
            for uci in best:
                if not solver.forces_mate(board, chess.Move.from_uci(uci), mate):
                    return False, None, f"{uci} erzwingt KEIN Matt in {mate}"
            shortest = solver.shortest_mate(board, mate)
            full = solver.all_mating_moves(board, mate)
        except TooExpensive as exc:
            return False, None, f"Verifikation zu teuer: {exc}"
        notes = [f"Matt in {mate} bestaetigt ({solver.nodes} Knoten)"]
        if shortest is not None and shortest < mate:
            notes.append(f"Hinweis: kuerzestes Matt ist {shortest}")
        if full != set(best):
            notes.append("akzeptiert erweitert um " + " ".join(sorted(full - set(best))))
        return True, full, "; ".join(notes)
    search = MaterialSearch()
    try:
        scores = search.root_scores(board, depth)
    except TooExpensive as exc:
        return False, None, f"Verifikation zu teuer: {exc}"
    static = search.material(board)
    acc = set(best)
    others = [s for u, s in scores.items() if u not in acc]
    best_other = max(others) if others else -MATE_SCORE
    worst_acc = min(scores[u] for u in acc)
    gain = worst_acc - static
    margin = worst_acc - best_other
    txt = f"Gewinn {gain:+d} cp, Abstand {margin:+d} cp (Tiefe {depth}, {search.nodes} Knoten)"
    if gain < MIN_GAIN:
        top = max((u for u in scores if u not in acc), key=lambda u: scores[u], default="-")
        return False, None, f"kein klarer Materialgewinn: {txt}; bester anderer Zug {top}"
    if margin < MIN_MARGIN:
        top = max((u for u in scores if u not in acc), key=lambda u: scores[u], default="-")
        return False, None, f"nicht klar bester Zug: {txt}; {top} erreicht {scores.get(top)}"
    return True, acc, txt


def verify_all(depth: int, verbose: bool = True):
    kept = []
    dropped = []
    t0 = time.perf_counter()
    for entry in POSITIONS:
        t1 = time.perf_counter()
        ok, acc, txt = verify_entry(entry, depth)
        dt = time.perf_counter() - t1
        if ok:
            kept.append((entry, acc))
            if verbose:
                print(f"  ok      {entry[0]} {entry[1]}: {txt} [{dt:.1f} s]")
        else:
            dropped.append(entry)
            print(f"  VERWORFEN {entry[0]} {entry[1]}: {txt}")
    print(f"Verifikation: {len(kept)} behalten, {len(dropped)} verworfen "
          f"[{time.perf_counter() - t0:.1f} s]")
    return kept, dropped


# --------------------------------------------------------------------------
# Engine-Lauf
# --------------------------------------------------------------------------
def start_engine(args) -> ud.Engine:
    eng = ud.Engine(args.engine).start()
    eng.uci(timeout=10.0)
    eng.set_options(ud.parse_opts(args.opts))
    eng.isready(timeout=15.0)
    return eng


def run_engine(kept, args) -> int:
    try:
        eng = start_engine(args)
    except EngineError as exc:
        print(f"Engine-Start fehlgeschlagen: {exc}")
        return 0
    print(f"Engine: {eng.name}  (movetime {args.movetime} ms)\n")
    passed = 0
    for (pid, theme, fen, best, mate), acc in kept:
        board = chess.Board(fen)
        detail = ""
        ok = False
        try:
            if eng is None or not eng.is_alive():
                eng = start_engine(args)
            eng.send("ucinewgame")
            eng.isready(timeout=10.0)
            eng.position(board)
            res = eng.search(f"go movetime {args.movetime}", timeout=args.movetime / 1000 + 5.0)
            score = res.last_score
            mv = ud.legal_move(board, res.bestmove)
            if mv is None:
                detail = f"illegaler bestmove '{res.bestmove}'"
            elif res.bestmove not in acc:
                detail = (f"{board.san(mv)} ({res.bestmove}) statt {fmt_moves(board, acc)}, "
                          f"Score {ud.format_score(score)}, Tiefe {res.max_depth}")
            elif mate and not (score and score[0] == "mate" and 0 < score[1] <= mate):
                seen = [i["score"] for i in res.infos if i.get("score", ("", 0))[0] == "mate"]
                detail = (f"{board.san(mv)} richtig, aber letzter Score {ud.format_score(score)} "
                          f"statt Matt <= {mate}" + (f" (zwischendurch {seen[-1]})" if seen else ""))
            else:
                ok = True
                detail = f"{board.san(mv)}, Score {ud.format_score(score)}, Tiefe {res.max_depth}"
        except EngineTimeout as exc:
            detail = f"Timeout: {exc}"
            eng.kill()
        except EngineCrashed as exc:
            detail = f"Absturz: {exc}"
        except EngineError as exc:
            detail = f"Engine-Fehler: {exc}"
        tag = "PASS" if ok else "FAIL"
        goal = f"Matt in {mate}" if mate else "Material"
        print(f"[{tag}] {pid} {theme} ({goal}): {detail}")
        if not ok and args.verbose and eng is not None:
            for l in eng.tail(12):
                print("        | " + l)
        passed += ok
    if eng is not None:
        eng.quit()
    return passed


def fmt_moves(board, ucis) -> str:
    out = []
    for u in sorted(ucis):
        try:
            out.append(board.san(chess.Move.from_uci(u)))
        except ValueError:
            out.append(u)
    return "/".join(out)


def main(argv=None) -> int:
    ud.setup_console()
    p = argparse.ArgumentParser(description="Taktik-Testsuite (mit python-chess-Verifikation)")
    ud.add_engine_argument(p)
    p.add_argument("--movetime", type=int, default=2000, help="ms pro Stellung (Default 2000)")
    p.add_argument("--opts", default="", help="Optionen, z.B. 'Threads=4,Hash=128'")
    p.add_argument("--verify-depth", type=int, default=4,
                   help="Tiefe der Material-Alpha-Beta-Verifikation (Default 4)")
    p.add_argument("--verify-only", action="store_true", help="nur verifizieren, keine Engine")
    p.add_argument("--only", default="", help="nur IDs, die diesen Text enthalten (z.B. M0)")
    p.add_argument("-v", "--verbose", action="store_true")
    args = p.parse_args(argv)

    global POSITIONS
    if args.only:
        POSITIONS = [e for e in POSITIONS if args.only.lower() in e[0].lower()]

    print(f"Verifiziere {len(POSITIONS)} Stellungen mit python-chess ...")
    kept, dropped = verify_all(args.verify_depth, verbose=args.verbose or args.verify_only)
    if len(kept) < 25 and not args.only:
        print(f"WARNUNG: nur {len(kept)} verifizierte Stellungen (Ziel >= 25)")
    if args.verify_only:
        return 0 if not dropped else 1
    if not kept:
        print("Keine Stellungen uebrig.")
        return 1
    print()
    passed = run_engine(kept, args)
    print()
    print(f"Ergebnis: {passed}/{len(kept)} bestanden"
          + (f" ({len(dropped)} Stellungen bei der Verifikation verworfen)" if dropped else ""))
    return 0 if passed == len(kept) else 1


if __name__ == "__main__":
    sys.exit(main())
