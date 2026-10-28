"""End-to-end harness for acreage: a private Xvfb display, the real release binary, xdotool
for input, and the ACREAGE_STATE_FILE hook for assertions.

Nothing here touches the user's session or files: every test builds its own fixture under
a temp dir, `xdg-open` and the terminal are replaced by recorders, and trashing happens
on /tmp (tmpfs), so it lands in /tmp/.Trash-$UID.
"""

import json
import os
import shutil
import signal
import subprocess
import tempfile
import time
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BIN = ROOT / "target" / "release" / "acreage"
SHOTS = Path(os.environ.get("ACREAGE_E2E_SHOTS", ROOT / "e2e" / "shots"))


class Display:
    """One Xvfb server shared by the whole run."""

    def __init__(self, w=1280, h=800):
        self.num = 90 + os.getpid() % 100
        self.proc = subprocess.Popen(
            ["Xvfb", f":{self.num}", "-screen", "0", f"{w}x{h}x24", "-nolisten", "tcp"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        self.env = dict(os.environ, DISPLAY=f":{self.num}")
        self.env.pop("WAYLAND_DISPLAY", None)
        deadline = time.time() + 10
        while time.time() < deadline:
            if subprocess.run(["xdpyinfo"], env=self.env, capture_output=True).returncode == 0:
                return
            time.sleep(0.1)
        raise RuntimeError("Xvfb did not start")

    def close(self):
        self.proc.terminate()
        self.proc.wait(5)


def fake_bin(dirpath: Path, record: Path):
    """xdg-open and a terminal that only record how they were called."""
    dirpath.mkdir(parents=True, exist_ok=True)
    for name in ["xdg-open", "fake-term"]:
        p = dirpath / name
        p.write_text(f'#!/bin/sh\necho "{name} $(pwd) $*" >> "{record}"\n')
        p.chmod(0o755)


class App:
    def __init__(self, display: Display, args, tmp: Path, size=(1200, 760), config_home=None, env_extra=None):
        self.display = display
        self.state_file = tmp / "state.json"
        self.record = tmp / "launched.txt"
        bindir = tmp / "bin"
        fake_bin(bindir, self.record)
        env = dict(display.env, ACREAGE_STATE_FILE=str(self.state_file), TERMINAL="fake-term")
        # Never read or write the real ~/.config/acreage.
        env["XDG_CONFIG_HOME"] = str(config_home or (tmp / "config"))
        # A private HOME too: reports default to ~/, and nothing may land in the real one.
        self.home = tmp / "home"
        self.home.mkdir(exist_ok=True)
        env["HOME"] = str(self.home)
        env.update(env_extra or {})
        env["PATH"] = f"{bindir}:{env['PATH']}"
        self.stderr = tmp / "stderr.txt"
        self.proc = subprocess.Popen([str(BIN), *args], env=env, stdout=subprocess.DEVNULL, stderr=open(self.stderr, "w"))
        self.wid = None
        deadline = time.time() + 15
        while time.time() < deadline and self.wid is None:
            r = self.xdo("search", "--onlyvisible", "--pid", str(self.proc.pid), check=False)
            if r.stdout.strip():
                self.wid = r.stdout.split()[0]
            else:
                time.sleep(0.1)
        if self.wid is None:
            raise RuntimeError("acreage window never appeared: " + self.stderr.read_text())
        self.xdo("windowsize", self.wid, str(size[0]), str(size[1]))
        self.xdo("windowmove", self.wid, "0", "0")
        self.focus()

    # ---- input
    def xdo(self, *a, check=True):
        return subprocess.run(["xdotool", *a], env=self.display.env, capture_output=True, text=True, check=check)

    def focus(self):
        self.xdo("windowfocus", "--sync", self.wid, check=False)

    def key(self, *keys, delay=0.15):
        for k in keys:
            self.xdo("key", "--window", self.wid, k)
            time.sleep(delay)

    def type(self, text):
        self.xdo("type", "--delay", "20", text)
        time.sleep(0.2)

    def move(self, x, y):
        self.xdo("mousemove", "--window", self.wid, str(int(x)), str(int(y)))
        time.sleep(0.15)

    def click(self, x, y, button=1, repeat=1):
        self.move(x, y)
        self.xdo("click", "--repeat", str(repeat), "--delay", "80", str(button))
        time.sleep(0.3)

    def click_block(self, path, button=1, repeat=1, at="center"):
        self.settle(quiet=0.25)
        x, y, w, h = self.rect(path)
        if at == "header":
            self.click(x + min(w / 2, 40), y + 6, button, repeat)
        else:
            self.click(x + w / 2, y + h / 2 + 4, button, repeat)

    def resize(self, w, h):
        self.xdo("windowsize", self.wid, str(w), str(h))
        time.sleep(0.5)

    # ---- state
    def state(self):
        for _ in range(20):
            try:
                return json.loads(self.state_file.read_text())
            except (FileNotFoundError, json.JSONDecodeError):
                time.sleep(0.05)
        raise AssertionError("no state file")

    def wait(self, pred, timeout=8.0, what="condition"):
        deadline = time.time() + timeout
        last = None
        while time.time() < deadline:
            if not self.alive():
                raise AssertionError(f"acreage exited while waiting for {what}: {self.stderr.read_text()[-2000:]}")
            try:
                last = self.state()
                if pred(last):
                    return last
            except AssertionError:
                pass
            time.sleep(0.1)
        raise AssertionError(f"timed out waiting for {what}; last state: {json.dumps(last, ensure_ascii=False)[:1500]}")

    def wait_done(self):
        self.wait(lambda s: s.get("screen") == "browse" and s["scan"] == "done", what="scan done")
        return self.settle()

    def settle(self, quiet=0.4, timeout=8.0):
        """Wait until the layout matches the window and stops changing (resizes and the
        post-scan relayout land a frame or two after the state that triggered them)."""
        w = int(self.xdo("getwindowgeometry", "--shell", self.wid).stdout.split("WIDTH=")[1].split()[0])
        deadline = time.time() + timeout
        prev, since = None, time.time()
        while time.time() < deadline:
            s = self.state()
            root = s.get("rects") and s["rects"][0].rsplit("|", 4)
            fits = bool(root) and abs(float(root[1]) + float(root[3]) - (w - 1)) < 2
            key = (tuple(s.get("rects", [])), s.get("animating"))
            if key != prev or not fits:
                prev, since = key, time.time()
            elif time.time() - since >= quiet:
                return s
            time.sleep(0.05)
        raise AssertionError("layout never settled")

    def rect(self, path):
        for r in self.state()["rects"]:
            p, x, y, w, h = r.rsplit("|", 4)
            if p == str(path):
                return float(x), float(y), float(w), float(h)
        raise AssertionError(f"no block for {path}")

    def alive(self):
        return self.proc.poll() is None

    def shot(self, name):
        SHOTS.mkdir(parents=True, exist_ok=True)
        subprocess.run(["import", "-window", self.wid, str(SHOTS / f"{name}.png")], env=self.display.env, capture_output=True)

    def close(self):
        if self.alive():
            self.proc.send_signal(signal.SIGTERM)
            try:
                self.proc.wait(5)
            except subprocess.TimeoutExpired:
                self.proc.kill()
        err = self.stderr.read_text()
        return err


DISPLAY = None


def display():
    global DISPLAY
    if DISPLAY is None:
        DISPLAY = Display()
    return DISPLAY


class AcreageCase(unittest.TestCase):
    """Base: a temp dir per test, the app closed after, and no panics allowed."""

    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="acreage-e2e-", dir="/tmp"))
        self.apps = []

    def tearDown(self):
        for a in self.apps:
            err = a.close()
            self.assertNotIn("panicked", err, f"acreage panicked:\n{err[-3000:]}")
        # Restore permissions so cleanup can't fail on locked fixtures.
        for p in self.tmp.rglob("*"):
            try:
                if p.is_dir() and not p.is_symlink():
                    p.chmod(0o755)
            except OSError:
                pass
        shutil.rmtree(self.tmp, ignore_errors=True)

    def launch(self, *args, size=(1200, 760), config_home=None, env=None):
        a = App(display(), [str(x) for x in args], self.tmp, size=size, config_home=config_home, env_extra=env)
        self.apps.append(a)
        return a

    def run_cli(self, *args):
        env = dict(display().env, XDG_CONFIG_HOME=str(self.tmp / "config"))
        return subprocess.run([str(BIN), *map(str, args)], env=env, capture_output=True, text=True, timeout=60)


def write(path: Path, size: int, sparse=True):
    """A file of `size` apparent bytes (sparse by default: cheap on tmpfs)."""
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "wb") as f:
        if size:
            if sparse:
                f.seek(size - 1)
                f.write(b"\0")
            else:
                f.write(os.urandom(size))
    return path


def apparent_total(root: Path):
    """Reference total like `du -sb --apparent-size`: dirs count too, hardlinks once,
    symlinks not followed, mount points not entered."""
    seen = set()
    total = 0  # acreage counts entries below the root, not the root's own inode
    for dirpath, dirnames, filenames in os.walk(root, followlinks=False, onerror=lambda e: None):
        for name in dirnames + filenames:
            st = os.lstat(os.path.join(dirpath, name))
            if st.st_nlink > 1 and not os.path.isdir(os.path.join(dirpath, name)):
                if (st.st_dev, st.st_ino) in seen:
                    continue
                seen.add((st.st_dev, st.st_ino))
            total += st.st_size
    return total


def trash_dir():
    return Path(f"/tmp/.Trash-{os.getuid()}/files")
