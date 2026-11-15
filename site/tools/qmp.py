#!/usr/bin/env python3
"""Drive a QEMU guest through QMP: screen dumps, a frame recorder, mouse
clicks and typed text. Used to make the pictures and the boot movie of the
page (see ../README.md).

    qmp.py SOCKET shot FILE.ppm
    qmp.py SOCKET record DIR FPS SECONDS      frames DIR/f000001.ppm ...
    qmp.py SOCKET click X Y [WIDTH HEIGHT]    pixel position on the screen
    qmp.py SOCKET move X Y [WIDTH HEIGHT]
    qmp.py SOCKET type 'text'                 US layout
    qmp.py SOCKET key ret|esc|tab|...         one QKeyCode, or a chord a-b
    qmp.py SOCKET quit
"""
import json
import os
import socket
import sys
import time


class Qmp:
    def __init__(self, path):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(path)
        self.file = self.sock.makefile("rw")
        self.file.readline()  # greeting
        self.cmd("qmp_capabilities")

    def cmd(self, name, **args):
        msg = {"execute": name}
        if args:
            msg["arguments"] = args
        self.file.write(json.dumps(msg) + "\n")
        self.file.flush()
        while True:
            answer = json.loads(self.file.readline())
            if "return" in answer:
                return answer["return"]
            if "error" in answer:
                raise RuntimeError(f"{name}: {answer['error']}")


SHIFTED = dict(zip('~!@#$%^&*()_+{}|:"<>?', "`1234567890-=[]\\;',./"))
NAMES = {
    " ": "spc", "-": "minus", "=": "equal", "[": "bracket_left", "]": "bracket_right",
    "\\": "backslash", ";": "semicolon", "'": "apostrophe", ",": "comma", ".": "dot",
    "/": "slash", "`": "grave_accent", "\n": "ret", "\t": "tab",
}


def keys_for(ch):
    shift = False
    if ch in SHIFTED:
        ch, shift = SHIFTED[ch], True
    elif ch.isalpha() and ch.isupper():
        ch, shift = ch.lower(), True
    code = NAMES.get(ch, ch)
    return (["shift"] if shift else []) + [code]


def send_keys(q, codes, hold_ms=40):
    q.cmd("send-key", keys=[{"type": "qcode", "data": c} for c in codes], **{"hold-time": hold_ms})
    time.sleep(hold_ms / 1000 + 0.03)


def pointer(q, x, y, w, h, click):
    # usb-tablet takes absolute positions on a 0..32767 scale.
    events = [
        {"type": "abs", "data": {"axis": "x", "value": round(x * 32767 / w)}},
        {"type": "abs", "data": {"axis": "y", "value": round(y * 32767 / h)}},
    ]
    q.cmd("input-send-event", events=events)
    if click:
        for down in (True, False):
            time.sleep(0.12)
            q.cmd("input-send-event", events=[{"type": "btn", "data": {"down": down, "button": "left"}}])


def main():
    q = Qmp(sys.argv[1])
    what, args = sys.argv[2], sys.argv[3:]
    if what == "shot":
        q.cmd("screendump", filename=os.path.abspath(args[0]))
    elif what == "record":
        out, fps, seconds = os.path.abspath(args[0]), float(args[1]), float(args[2])
        os.makedirs(out, exist_ok=True)
        start = time.monotonic()
        n = 0
        while time.monotonic() - start < seconds:
            n += 1
            q.cmd("screendump", filename=f"{out}/f{n:06d}.ppm")
            # Keep to the frame clock; a slow dump does not shift later frames.
            wait = start + n / fps - time.monotonic()
            if wait > 0:
                time.sleep(wait)
        print(f"{n} frames in {time.monotonic() - start:.1f} s")
    elif what in ("click", "move"):
        x, y = float(args[0]), float(args[1])
        w, h = (float(args[2]), float(args[3])) if len(args) > 3 else (1920.0, 1080.0)
        pointer(q, x, y, w, h, what == "click")
    elif what == "type":
        for ch in args[0]:
            send_keys(q, keys_for(ch))
    elif what == "key":
        send_keys(q, args[0].split("-"))
    elif what == "quit":
        q.cmd("quit")
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
