"""Minimale UCI-Mock-Engine (Zufallszuege via python-chess) zum Testen des Harness.

Unterstuetzt: uci, isready (auch waehrend der Suche), setoption, ucinewgame,
position startpos|fen ... [moves ...], go (wtime/btime/winc/binc/movestogo/
movetime/depth/nodes/infinite/ponder), stop, ponderhit, quit, sowie d, perft N, bench.

Spielstaerke: Matt-in-1 wird gespielt (mit 'score mate 1'), sonst ein Zufallszug.

Fuer Negativtests des Harness kann per Umgebungsvariable ein Fehler eingebaut werden:
  MOCK_ENGINE_BUG=illegal|slow|crash|hang|nostop|badpv|no0000
  MOCK_ENGINE_BUG_AFTER=N   (Fehler ab dem N-ten go, Default 1)
"""
import os
import random
import sys
import threading
import time

import chess

now = time.perf_counter

BUG = os.environ.get("MOCK_ENGINE_BUG", "").strip().lower()
BUG_AFTER = int(os.environ.get("MOCK_ENGINE_BUG_AFTER", "1") or 1)

OUT_LOCK = threading.Lock()


def out(line: str) -> None:
    with OUT_LOCK:
        sys.stdout.write(line + "\n")
        sys.stdout.flush()


class Mock:
    def __init__(self):
        self.board = chess.Board()
        self.rng = random.Random()
        self.options = {"hash": 256, "threads": 8, "move overhead": 30, "ponder": False}
        self.thread = None
        self.stop_event = threading.Event()
        self.ponderhit_time = None  # gesetzt durch 'ponderhit'
        self.go_count = 0

    # ------------------------------------------------------------------
    def cmd_uci(self):
        out("id name Turmfalke Mock 0.1")
        out("id author Harness")
        out("option name Hash type spin default 256 min 1 max 65536")
        out("option name Threads type spin default 8 min 1 max 256")
        out("option name Move Overhead type spin default 30 min 0 max 5000")
        out("option name Clear Hash type button")
        out("option name Ponder type check default false")
        out("uciok")

    def cmd_setoption(self, tok):
        try:
            n = tok.index("name")
        except ValueError:
            return
        if "value" in tok:
            v = tok.index("value")
            name = " ".join(tok[n + 1:v]).lower()
            value = " ".join(tok[v + 1:])
        else:
            name = " ".join(tok[n + 1:]).lower()
            value = None
        if name in ("hash", "threads", "move overhead"):
            try:
                self.options[name] = int(value)
            except (TypeError, ValueError):
                pass
        elif name == "ponder":
            self.options["ponder"] = (value or "").lower() == "true"

    def cmd_position(self, tok):
        if len(tok) < 2:
            return
        try:
            if tok[1] == "startpos":
                board = chess.Board()
                rest = tok[2:]
            elif tok[1] == "fen":
                end = tok.index("moves") if "moves" in tok else len(tok)
                board = chess.Board(" ".join(tok[2:end]))
                rest = tok[end:]
            else:
                return
            if rest and rest[0] == "moves":
                for uci in rest[1:]:
                    board.push_uci(uci)
            self.board = board
        except ValueError as exc:
            out(f"info string ungueltige Position: {exc}")

    # ------------------------------------------------------------------
    def cmd_go(self, tok):
        self.join_search()
        self.go_count += 1
        bug = BUG if self.go_count >= BUG_AFTER else ""
        if bug == "crash":
            os._exit(3)
        params = {}
        flags = set()
        i = 1
        while i < len(tok):
            t = tok[i]
            if t in ("infinite", "ponder"):
                flags.add(t)
                i += 1
            elif t == "searchmoves":
                i = len(tok)
            else:
                if i + 1 < len(tok):
                    try:
                        params[t] = int(tok[i + 1])
                    except ValueError:
                        pass
                i += 2
        self.stop_event.clear()
        self.ponderhit_time = None
        board = self.board.copy()
        self.thread = threading.Thread(target=self.search, args=(board, params, flags, bug),
                                       daemon=True)
        self.thread.start()

    def budget_ms(self, board, params, bug):
        if "movetime" in params:
            return params["movetime"]
        mine = params.get("wtime" if board.turn == chess.WHITE else "btime")
        if mine is not None:
            inc = params.get("winc" if board.turn == chess.WHITE else "binc", 0)
            mtg = params.get("movestogo", 30) or 30
            if bug == "slow":
                return mine + 300
            overhead = self.options["move overhead"]
            budget = mine / mtg + inc * 0.75
            return max(0.0, min(budget, (mine - overhead) * 0.8))
        if "depth" in params:
            return 5 * params["depth"]
        if "nodes" in params:
            return 50
        return None  # wie infinite

    def choose(self, board):
        legal = list(board.legal_moves)
        if not legal:
            return None, None
        for m in legal:
            board.push(m)
            mate = board.is_checkmate()
            board.pop()
            if mate:
                return m, "mate 1"
        return self.rng.choice(legal), f"cp {self.rng.randint(-30, 30)}"

    def search(self, board, params, flags, bug):
        t0 = now()
        best, score = self.choose(board)
        infinite = "infinite" in flags
        ponder = "ponder" in flags
        budget = self.budget_ms(board, params, bug)
        if budget is None:
            infinite = True
        deadline = None if (infinite or ponder) else t0 + budget / 1000.0
        if bug == "hang":
            return  # nie antworten
        max_depth = params.get("depth")
        depth = 0
        last_info = 0.0
        while True:
            if self.stop_event.is_set():
                if bug == "nostop" and (infinite or ponder) and now() - t0 < 2.0:
                    time.sleep(0.01)
                else:
                    break
            if ponder and self.ponderhit_time is not None:
                ponder = False
                b = self.budget_ms(board, params, bug)
                deadline = self.ponderhit_time + (b or 0) / 1000.0
            t = now()
            if deadline is not None and t >= deadline:
                break
            if max_depth is not None and depth >= max_depth:
                break
            if "nodes" in params and depth * 1000 >= params["nodes"]:
                break
            if best is not None and (t - last_info > 0.05 or depth < 3):
                depth += 1
                last_info = t
                self.emit_info(board, best, score, depth, t0, bug)
            wait = 0.01
            if deadline is not None:
                wait = max(0.0, min(wait, deadline - now()))
            self.stop_event.wait(wait)
        if best is None:
            if bug == "no0000":
                out("bestmove e2e4")
            else:
                out("info depth 0 score " + ("mate 0" if board.is_check() else "cp 0"))
                out("bestmove 0000")
            return
        if bug == "illegal":
            out("bestmove a1a1")
            return
        board.push(best)
        replies = list(board.legal_moves)
        if replies:
            out(f"bestmove {best.uci()} ponder {self.rng.choice(replies).uci()}")
        else:
            out(f"bestmove {best.uci()}")

    def emit_info(self, board, best, score, depth, t0, bug):
        pv = [best]
        b = board.copy(stack=False)
        b.push(best)
        for _ in range(3):
            moves = list(b.legal_moves)
            if not moves or score == "mate 1":
                break
            m = self.rng.choice(moves)
            pv.append(m)
            b.push(m)
        pv_str = " ".join(m.uci() for m in pv)
        if bug == "badpv":
            pv_str += " a1a1"
        ms = int((now() - t0) * 1000)
        nodes = 1000 * depth
        nps = int(nodes * 1000 / max(ms, 1))
        out(f"info depth {depth} seldepth {depth} multipv 1 score {score} nodes {nodes} "
            f"nps {nps} hashfull 0 time {ms} pv {pv_str}")

    def join_search(self):
        if self.thread is not None:
            self.stop_event.set()
            self.thread.join(timeout=5)
            self.thread = None

    # ------------------------------------------------------------------
    def perft(self, board, depth):
        if depth == 0:
            return 1
        if depth == 1:
            return board.legal_moves.count()
        total = 0
        for m in board.legal_moves:
            board.push(m)
            total += self.perft(board, depth - 1)
            board.pop()
        return total

    def cmd_perft(self, tok):
        depth = int(tok[1]) if len(tok) > 1 else 1
        board = self.board.copy()
        total = 0
        for m in board.legal_moves:
            board.push(m)
            n = self.perft(board, depth - 1)
            board.pop()
            total += n
            out(f"{m.uci()}: {n}")
        out("")
        out(f"Nodes searched: {total}")

    def cmd_d(self):
        out("")
        for row in str(self.board).splitlines():
            out(" " + row)
        out("")
        out(f"Fen: {self.board.fen()}")

    # ------------------------------------------------------------------
    def run(self):
        while True:
            line = sys.stdin.readline()
            if not line:
                break
            tok = line.split()
            if not tok:
                continue
            c = tok[0]
            if c == "uci":
                self.cmd_uci()
            elif c == "isready":
                out("readyok")
            elif c == "setoption":
                self.cmd_setoption(tok)
            elif c == "ucinewgame":
                self.join_search()
                self.board = chess.Board()
            elif c == "position":
                self.join_search()
                self.cmd_position(tok)
            elif c == "go":
                self.cmd_go(tok)
            elif c == "stop":
                self.stop_event.set()
            elif c == "ponderhit":
                self.ponderhit_time = now()
            elif c == "quit":
                self.stop_event.set()
                break
            elif c == "d":
                self.cmd_d()
            elif c == "perft":
                self.join_search()
                self.cmd_perft(tok)
            elif c == "bench":
                out("info string bench (mock)")
                out("123456 nodes 98765 nps")
            else:
                out(f"info string unbekannter Befehl: {line.strip()}")
        os._exit(0)


if __name__ == "__main__":
    Mock().run()
