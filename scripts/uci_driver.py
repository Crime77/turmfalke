"""Gemeinsamer UCI-Treiber fuer die Turmfalke-Testskripte.

Startet eine Engine als Subprozess (Pipes, Textmodus, zeilengepuffert), liest
stdout/stderr in Hintergrund-Threads in eine Queue und bietet:

* ``send(cmd)``                 - Befehl schicken (LF-terminiert, sofort geflusht)
* ``wait_for(prefix, timeout)`` - bis zu einer Zeile warten, deren erste Tokens
                                  ``prefix`` entsprechen; liefert (Zeile, alle
                                  bis dahin gelesenen Zeilen ohne die Treffer-Zeile)
* ``drain(dauer)``              - alle Zeilen einer Zeitspanne einsammeln
* ``quit()``                    - sauber beenden (``quit``, danach kill)

Absturz (Prozessende / EOF) -> ``EngineCrashed``, Haenger -> ``EngineTimeout``.
Beide Ausnahmen tragen die bis dahin gelesenen Zeilen in ``.lines``.

Nur Python-Stdlib + python-chess.
"""
from __future__ import annotations

import argparse
import collections
import io
import os
import queue
import re
import subprocess
import sys
import threading
import time
from dataclasses import dataclass, field
from typing import Iterable, Optional

import chess

SCRIPTS_DIR = os.path.dirname(os.path.abspath(__file__))
DEFAULT_ENGINE = os.path.normpath(
    os.path.join(SCRIPTS_DIR, "..", "target", "release", "turmfalke.exe"))

now = time.perf_counter  # hochaufloesende, monotone Uhr (QPC unter Windows)

MOVE_RE = re.compile(r"^(?:[a-h][1-8][a-h][1-8][qrbn]?|0000)$")


# --------------------------------------------------------------------------
# Ausnahmen
# --------------------------------------------------------------------------
class EngineError(Exception):
    def __init__(self, msg: str, lines: Optional[Iterable[str]] = None):
        super().__init__(msg)
        self.lines = list(lines or [])


class EngineStartError(EngineError):
    pass


class EngineTimeout(EngineError):
    pass


class EngineCrashed(EngineError):
    pass


# --------------------------------------------------------------------------
# Hilfsfunktionen
# --------------------------------------------------------------------------
def setup_console() -> None:
    """Konsole robust machen (keine UnicodeErrors, zeilenweise Ausgabe)."""
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(errors="replace", line_buffering=True)
        except (AttributeError, ValueError):
            pass


def engine_command(path: str) -> list:
    """Kommandozeile fuer eine Engine; ``*.py`` wird per sys.executable gestartet."""
    if path.lower().endswith(".py"):
        return [sys.executable, "-u", path]
    return [path]


def add_engine_argument(parser: argparse.ArgumentParser, flag: str = "--engine",
                        dest: Optional[str] = None, help_text: Optional[str] = None) -> None:
    kwargs = {"default": DEFAULT_ENGINE,
              "help": help_text or f"Pfad zur Engine (Default: {DEFAULT_ENGINE}); *.py wird mit Python gestartet"}
    if dest:
        kwargs["dest"] = dest
    parser.add_argument(flag, **kwargs)


def parse_opts(spec: str) -> list:
    """'Threads=1,Hash=64,Move Overhead=50' -> [('Threads','1'), ...]. Ohne '=' -> Button."""
    result = []
    for part in (spec or "").split(","):
        part = part.strip()
        if not part:
            continue
        if "=" in part:
            name, value = part.split("=", 1)
            result.append((name.strip(), value.strip()))
        else:
            result.append((part, None))
    return result


def position_command(board: chess.Board) -> str:
    """'position startpos|fen ... moves ...' fuer ein python-chess-Brett inkl. Zugliste."""
    root = board.root()
    moves = [m.uci() for m in board.move_stack]
    if root.fen() == chess.STARTING_FEN:
        cmd = "position startpos"
    else:
        cmd = "position fen " + root.fen()
    if moves:
        cmd += " moves " + " ".join(moves)
    return cmd


def legal_move(board: chess.Board, uci: Optional[str]) -> Optional[chess.Move]:
    """Strikte Pruefung: nur exakte UCI-Notation (e1g1, e7e8q) eines legalen Zugs."""
    if not uci or not MOVE_RE.match(uci) or uci == "0000":
        return None
    try:
        move = chess.Move.from_uci(uci)
    except ValueError:
        return None
    return move if move in board.legal_moves else None


def validate_pv(board: chess.Board, pv: list) -> tuple:
    """(ok, index_des_ersten_illegalen_zugs, zug). Arbeitet auf einer Kopie."""
    b = board.copy(stack=False)
    for i, uci in enumerate(pv):
        mv = legal_move(b, uci)
        if mv is None:
            return False, i, uci
        b.push(mv)
    return True, -1, None


# --------------------------------------------------------------------------
# Parser
# --------------------------------------------------------------------------
_INT_FIELDS = {"depth", "seldepth", "multipv", "nodes", "nps", "hashfull", "tbhits",
               "time", "currmovenumber", "cpuload", "sbhits"}
_MOVELIST_FIELDS = {"pv", "refutation", "currline"}
_ALL_KEYS = _INT_FIELDS | _MOVELIST_FIELDS | {"score", "currmove", "string", "wdl"}


def parse_info(line: str) -> dict:
    """Parst eine 'info ...'-Zeile.

    Ergebnis-Keys (soweit vorhanden): depth, seldepth, multipv, nodes, nps, hashfull,
    time, score=('cp'|'mate', int), bound='lower'|'upper', pv=[...], currmove,
    string, wdl. ``errors`` listet unparsbare Felder (leer = ok).
    """
    tok = line.split()
    info: dict = {"errors": []}
    if not tok or tok[0] != "info":
        info["errors"].append("keine info-Zeile")
        return info
    i = 1
    n = len(tok)
    while i < n:
        key = tok[i]
        if key in _INT_FIELDS:
            if i + 1 < n and re.fullmatch(r"-?\d+", tok[i + 1]):
                info[key] = int(tok[i + 1])
            else:
                info["errors"].append(f"{key} ohne Zahl")
            i += 2
        elif key == "score":
            kind = tok[i + 1] if i + 1 < n else None
            val = tok[i + 2] if i + 2 < n else None
            if kind in ("cp", "mate") and val is not None and re.fullmatch(r"[+-]?\d+", val):
                info["score"] = (kind, int(val))
            else:
                info["errors"].append(f"score unparsbar: {' '.join(tok[i:i + 3])}")
            i += 3
            while i < n and tok[i] in ("lowerbound", "upperbound"):
                info["bound"] = "lower" if tok[i] == "lowerbound" else "upper"
                i += 1
        elif key in _MOVELIST_FIELDS:
            j = i + 1
            moves = []
            while j < n and tok[j] not in _ALL_KEYS:
                moves.append(tok[j])
                j += 1
            bad = [m for m in moves if not MOVE_RE.match(m)]
            if bad:
                info["errors"].append(f"{key} enthaelt Nicht-Zuege: {' '.join(bad[:3])}")
            if not moves and key == "pv":
                info["errors"].append("leere pv")
            info[key] = moves
            i = j
        elif key == "currmove":
            if i + 1 < n:
                info["currmove"] = tok[i + 1]
            i += 2
        elif key == "wdl":
            vals = tok[i + 1:i + 4]
            if len(vals) == 3 and all(v.isdigit() for v in vals):
                info["wdl"] = tuple(int(v) for v in vals)
            else:
                info["errors"].append("wdl unparsbar")
            i += 4
        elif key == "string":
            info["string"] = " ".join(tok[i + 1:])
            break
        else:
            info.setdefault("unknown", []).append(key)
            i += 1
    return info


def parse_bestmove(line: str) -> tuple:
    """'bestmove e2e4 ponder e7e5' -> ('e2e4', 'e7e5'); fehlende Teile -> None."""
    tok = line.split()
    best = tok[1] if len(tok) > 1 else None
    ponder = None
    if len(tok) > 3 and tok[2] == "ponder":
        ponder = tok[3]
    return best, ponder


def parse_option(line: str) -> Optional[dict]:
    """'option name Move Overhead type spin default 30 min 0 max 5000' -> dict."""
    tok = line.split()
    if len(tok) < 2 or tok[0] != "option" or tok[1] != "name":
        return None
    keys = {"type", "default", "min", "max", "var"}
    opt: dict = {"name": None, "type": None, "default": None, "min": None, "max": None, "vars": []}
    # Name: alles zwischen 'name' und 'type'
    try:
        t = tok.index("type", 2)
    except ValueError:
        opt["name"] = " ".join(tok[2:])
        return opt
    opt["name"] = " ".join(tok[2:t])
    i = t
    while i < len(tok):
        key = tok[i]
        j = i + 1
        vals = []
        while j < len(tok) and tok[j] not in keys:
            vals.append(tok[j])
            j += 1
        val = " ".join(vals)
        if key == "var":
            opt["vars"].append(val)
        elif key in keys:
            opt[key] = val
        i = j
    return opt


# --------------------------------------------------------------------------
# Ergebnis-Container
# --------------------------------------------------------------------------
@dataclass
class UciInfo:
    name: Optional[str] = None
    author: Optional[str] = None
    options: dict = field(default_factory=dict)  # name -> option dict
    lines: list = field(default_factory=list)
    elapsed_ms: float = 0.0


@dataclass
class SearchResult:
    bestmove: Optional[str]
    ponder: Optional[str]
    infos: list
    lines: list
    elapsed_ms: float
    bestmove_line: str = ""

    @property
    def last_score(self):
        for info in reversed(self.infos):
            if "score" in info:
                return info["score"]
        return None

    @property
    def max_depth(self) -> int:
        return max((i.get("depth", 0) for i in self.infos), default=0)

    @property
    def last_info_with_pv(self):
        for info in reversed(self.infos):
            if info.get("pv"):
                return info
        return None


def format_score(score) -> str:
    if score is None:
        return "-"
    kind, val = score
    if kind == "mate":
        return f"#{val}"
    return f"{val / 100:+.2f}"


# --------------------------------------------------------------------------
# Engine
# --------------------------------------------------------------------------
_EOF = object()


class Engine:
    def __init__(self, path: str, name: Optional[str] = None, log_size: int = 500):
        self.path = path
        self.name = name or os.path.basename(path)
        self.cmd = engine_command(path)
        self.proc: Optional[subprocess.Popen] = None
        self._stdin = None
        self._q: "queue.Queue" = queue.Queue()
        self._log = collections.deque(maxlen=log_size)
        self._log_lock = threading.Lock()
        self._send_lock = threading.Lock()
        self._eof = False
        self._t_start = now()
        self.last_send_time: Optional[float] = None
        self.last_match_time: Optional[float] = None
        self.uci_info: Optional[UciInfo] = None

    # ---- Lebenszyklus -----------------------------------------------------
    def start(self) -> "Engine":
        if not os.path.isfile(self.path):
            raise EngineStartError(f"Engine nicht gefunden: {self.path}")
        flags = 0
        if os.name == "nt":
            flags = getattr(subprocess, "CREATE_NO_WINDOW", 0)
        try:
            self.proc = subprocess.Popen(
                self.cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                stderr=subprocess.PIPE, creationflags=flags)
        except OSError as exc:
            raise EngineStartError(f"Engine-Start fehlgeschlagen ({self.path}): {exc}")
        # Textmodus, LF als Zeilenende (wie die meisten GUIs), Flush pro Zeile.
        self._stdin = io.TextIOWrapper(self.proc.stdin, encoding="utf-8", errors="replace",
                                       newline="\n", line_buffering=True, write_through=True)
        out = io.TextIOWrapper(self.proc.stdout, encoding="utf-8", errors="replace")
        err = io.TextIOWrapper(self.proc.stderr, encoding="utf-8", errors="replace")
        self._t_start = now()
        threading.Thread(target=self._reader, args=(out,), daemon=True,
                         name=f"uci-out-{self.name}").start()
        threading.Thread(target=self._err_reader, args=(err,), daemon=True,
                         name=f"uci-err-{self.name}").start()
        return self

    def _logappend(self, direction: str, line: str, t: Optional[float] = None) -> None:
        with self._log_lock:
            self._log.append(((t or now()) - self._t_start, direction, line))

    def _reader(self, stream) -> None:
        try:
            for raw in iter(stream.readline, ""):
                t = now()
                line = raw.rstrip("\r\n")
                self._logappend("<", line, t)
                self._q.put((t, line))
        except (OSError, ValueError):
            pass
        finally:
            self._q.put((now(), _EOF))

    def _err_reader(self, stream) -> None:
        try:
            for raw in iter(stream.readline, ""):
                self._logappend("!", raw.rstrip("\r\n"))
        except (OSError, ValueError):
            pass

    def is_alive(self) -> bool:
        return self.proc is not None and self.proc.poll() is None

    @property
    def returncode(self):
        return None if self.proc is None else self.proc.poll()

    def _crash_message(self) -> str:
        rc = None
        if self.proc is not None:
            try:
                rc = self.proc.wait(timeout=1.0)
            except subprocess.TimeoutExpired:
                rc = None
        if rc is None:
            return "Engine hat stdout geschlossen (Prozess laeuft noch?)"
        return f"Engine-Prozess beendet (Exit-Code {rc})"

    def kill(self) -> None:
        if self.proc is not None and self.proc.poll() is None:
            try:
                self.proc.kill()
            except OSError:
                pass
            try:
                self.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                pass

    def quit(self, timeout: float = 2.0) -> bool:
        """'quit' schicken, auf Prozessende warten, notfalls killen.
        Rueckgabe True, wenn die Engine sich innerhalb ``timeout`` selbst beendet hat."""
        if self.proc is None:
            return True
        clean = True
        if self.proc.poll() is None:
            try:
                self.send("quit")
            except EngineError:
                pass
            try:
                self.proc.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                clean = False
                self.kill()
        self._close_pipes()
        return clean

    def _close_pipes(self) -> None:
        for s in (self._stdin,):
            try:
                if s is not None:
                    s.close()
            except (OSError, ValueError):
                pass

    def __enter__(self):
        return self.start() if self.proc is None else self

    def __exit__(self, *exc):
        self.quit()
        return False

    # ---- I/O --------------------------------------------------------------
    def send(self, cmd: str) -> float:
        """Befehl senden; Rueckgabe: Zeitpunkt (perf_counter) direkt nach dem Flush."""
        if self.proc is None or self._stdin is None:
            raise EngineCrashed("Engine nicht gestartet")
        with self._send_lock:
            try:
                self._stdin.write(cmd + "\n")
                self._stdin.flush()
            except (OSError, ValueError) as exc:
                raise EngineCrashed(f"Schreiben an Engine fehlgeschlagen ({exc}); "
                                    + self._crash_message())
            t = now()
        self.last_send_time = t
        self._logappend(">", cmd, t)
        return t

    def _get(self, deadline: float):
        """Naechste Zeile (t, line) oder None bei Timeout; EngineCrashed bei EOF."""
        if self._eof:
            raise EngineCrashed(self._crash_message())
        remaining = deadline - now()
        try:
            if remaining > 0:
                item = self._q.get(timeout=remaining)
            else:
                item = self._q.get_nowait()
        except queue.Empty:
            return None
        if item[1] is _EOF:
            self._eof = True
            raise EngineCrashed(self._crash_message())
        return item

    def wait_for(self, prefix: str, timeout: float) -> tuple:
        """Wartet auf eine Zeile, deren erste Tokens == prefix.split().
        Rueckgabe (zeile, gelesene_zeilen_davor). Zeitstempel der Zeile: last_match_time."""
        want = prefix.split()
        k = len(want)
        deadline = now() + timeout
        lines = []
        while True:
            try:
                item = self._get(deadline)
            except EngineCrashed as exc:
                exc.lines = lines
                raise
            if item is None:
                raise EngineTimeout(f"Timeout ({timeout:.2f} s) beim Warten auf '{prefix}'", lines)
            t, line = item
            if line.split()[:k] == want:
                self.last_match_time = t
                return line, lines
            lines.append(line)

    def drain(self, duration: float = 0.0) -> list:
        """Alle Zeilen einsammeln, die innerhalb ``duration`` Sekunden eintreffen."""
        deadline = now() + duration
        lines = []
        while True:
            try:
                item = self._get(deadline)
            except EngineCrashed as exc:
                exc.lines = lines
                raise
            if item is None:
                return lines
            lines.append(item[1])

    def tail(self, n: int = 20, width: Optional[int] = 160) -> list:
        """Letzte n Log-Zeilen ('>' gesendet, '<' empfangen, '!' stderr), optional gekuerzt."""
        with self._log_lock:
            items = list(self._log)[-n:]
        out = []
        for t, d, line in items:
            if width and len(line) > width:
                line = line[:width - 3] + "..."
            out.append(f"{t:9.3f} {d} {line}")
        return out

    # ---- UCI-Komfort ------------------------------------------------------
    def uci(self, timeout: float = 10.0) -> UciInfo:
        t0 = self.send("uci")
        _, lines = self.wait_for("uciok", timeout)
        info = UciInfo(lines=lines, elapsed_ms=(self.last_match_time - t0) * 1000)
        for line in lines:
            tok = line.split()
            if len(tok) >= 2 and tok[0] == "id":
                if tok[1] == "name":
                    info.name = " ".join(tok[2:])
                elif tok[1] == "author":
                    info.author = " ".join(tok[2:])
            elif tok and tok[0] == "option":
                opt = parse_option(line)
                if opt and opt["name"]:
                    info.options[opt["name"]] = opt
        self.uci_info = info
        if info.name:
            self.name = info.name
        return info

    def isready(self, timeout: float = 5.0) -> float:
        """isready -> readyok; Rueckgabe Latenz in ms."""
        t0 = self.send("isready")
        _, self.last_lines = self.wait_for("readyok", timeout)
        return (self.last_match_time - t0) * 1000

    def setoption(self, name: str, value=None) -> None:
        if value is None:
            self.send(f"setoption name {name}")
        else:
            self.send(f"setoption name {name} value {value}")

    def set_options(self, opts: Iterable) -> None:
        for name, value in opts:
            self.setoption(name, value)

    def position(self, board: chess.Board) -> None:
        self.send(position_command(board))

    def search(self, go_cmd: str, timeout: float) -> SearchResult:
        """go-Befehl senden und bis 'bestmove' sammeln (Zeitmessung ab go-Flush)."""
        t0 = self.send(go_cmd)
        line, lines = self.wait_for("bestmove", timeout)
        return self.make_result(line, lines, t0)

    def make_result(self, line: str, lines: list, t0: float) -> SearchResult:
        best, ponder = parse_bestmove(line)
        infos = [parse_info(l) for l in lines if l.startswith("info")]
        return SearchResult(best, ponder, infos, lines, (self.last_match_time - t0) * 1000, line)
