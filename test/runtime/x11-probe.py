#!/usr/bin/env python3
"""Map a window, ask the WM for fullscreen, print the resulting size.

Used by test-labwc-runtime.sh against a headless labwc with two outputs.
With --grab it also takes an X11 keyboard grab, which rootless Xwayland
forwards to the compositor as zwp_xwayland_keyboard_grab_v1.
"""
import sys
import time

from Xlib import X, display
from Xlib.protocol import event

grab = "--grab" in sys.argv

d = display.Display()
screen = d.screen()
win = screen.root.create_window(
    0, 0, 400, 300, 0, screen.root_depth,
    X.InputOutput, X.CopyFromParent,
    event_mask=X.StructureNotifyMask)
win.set_wm_name("thinfloe-x11-probe")
win.set_wm_class("thinfloe-probe", "thinfloe-probe")
win.map()
d.sync()
time.sleep(1)

if grab:
    win.grab_keyboard(False, X.GrabModeAsync, X.GrabModeAsync, X.CurrentTime)
    d.sync()
    time.sleep(1)

net_wm_state = d.intern_atom("_NET_WM_STATE")
fullscreen = d.intern_atom("_NET_WM_STATE_FULLSCREEN")
# _NET_WM_STATE_ADD = 1, source = 1 (normal application)
d.screen().root.send_event(
    event.ClientMessage(window=win, client_type=net_wm_state,
                        data=(32, [1, fullscreen, 0, 1, 0])),
    event_mask=X.SubstructureRedirectMask | X.SubstructureNotifyMask)
d.sync()
time.sleep(2)

geom = win.get_geometry()
print("GEOMETRY %dx%d" % (geom.width, geom.height))
