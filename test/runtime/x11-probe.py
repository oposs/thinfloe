#!/usr/bin/env python3
"""Map a window, ask the WM for fullscreen, print the resulting size.

Used by test-labwc-runtime.sh against a headless labwc with two outputs.
With --grab it also takes an X11 keyboard grab, which rootless Xwayland
forwards to the compositor as zwp_xwayland_keyboard_grab_v1.
With --keys SECONDS it prints "KEY <keysym name>" for every key press the
window gets for that long, then exits without the fullscreen step.
With --keymap KEYSYM,KEYSYM,... it first sets X keycodes 9, 10, ... to these
keysyms: wtype gives the keys it sends the codes 1, 2, ... (X 9, 10, ...) in
a keymap of its own, but Xwayland in this rig keeps its default evdev keymap
(xkbcomp -xkb $DISPLAY shows <ESC> = 9 after wtype's keymap arrived), so the
names would otherwise be Escape, 1, ...
With --background RRGGBB the window is filled with that colour.
With --buttons it prints "BUTTON <x> <y>" (window coordinates) for every
mouse button press, and with --hold SECONDS it stays fullscreen that long
after printing GEOMETRY.
With --fullscreen and --keys the window asks for fullscreen before it
prints the keys.
With --record SECONDS it maps no window: it prints what any client makes
the X server do for that long, through the RECORD extension, whichever
window would get it: "KEY <keysym name>" for a key press, "KEYUP <keysym
name>" for its release, "BUTTON <n>" and "BUTTONUP <n>" for a button,
"MOTION <x> <y>" (root coordinates) for pointer motion, and "XTEST <type>
<detail>" for each XTestFakeInput request a client makes (type 2/3 key
press/release, 4/5 button press/release, 6 motion), whether or not the
server turns it into an event.
The Deskflow client injects what it receives that way (XTest).
"""
import sys
import time

from Xlib import X, XK, display
from Xlib.protocol import event

grab = "--grab" in sys.argv
keys = None
if "--keys" in sys.argv:
    keys = float(sys.argv[sys.argv.index("--keys") + 1])
background = None
if "--background" in sys.argv:
    background = int(sys.argv[sys.argv.index("--background") + 1], 16)
buttons = "--buttons" in sys.argv
fullscreen_keys = "--fullscreen" in sys.argv
hold = 0.0
if "--hold" in sys.argv:
    hold = float(sys.argv[sys.argv.index("--hold") + 1])
keymap = []
if "--keymap" in sys.argv:
    keymap = sys.argv[sys.argv.index("--keymap") + 1].split(",")


def keysym_name(keysym):
    # XK.keysym_to_string() gives the character, not the name, and knows no
    # XF86 keysyms; python-xlib calls those XK_XF86_<name>, X calls them
    # XF86<name>.
    for attr in dir(XK):
        if attr.startswith("XK_") and getattr(XK, attr) == keysym:
            name = attr[3:]
            return "XF86" + name[5:] if name.startswith("XF86_") else name
    return "0x%x" % keysym


def record(seconds):
    import os
    import threading

    from Xlib.ext import record as rec
    from Xlib.protocol import rq

    ctl = display.Display()
    names = display.Display()
    XK.load_keysym_group("xf86")
    xtest = ctl.query_extension("XTEST").major_opcode
    ctx = ctl.record_create_context(0, [rec.AllClients], [{
        "core_requests": (0, 0), "core_replies": (0, 0),
        "ext_requests": (xtest, xtest, 2, 2), "ext_replies": (0, 0, 0, 0),
        "delivered_events": (0, 0),
        "device_events": (X.KeyPress, X.MotionNotify),
        "errors": (0, 0), "client_started": False, "client_died": False}])

    def seen(reply):
        if reply.category == rec.FromClient and not reply.client_swapped:
            # XTestFakeInput requests, several to a reply: type, detail.
            d = bytearray(reply.data)
            while len(d) > 5:
                n = (d[2] | d[3] << 8) * 4 or len(d)
                if d[0] == xtest:
                    print("XTEST %d %d" % (d[4], d[5]), flush=True)
                d = d[n:]
            return
        if reply.category != rec.FromServer or reply.client_swapped:
            return
        data = reply.data
        while len(data):
            e, data = rq.EventField(None).parse_binary_value(
                data, ctl.display, None, None)
            if e.type == X.KeyPress:
                # The client may change the keyboard mapping: ask the server.
                sym = names.get_keyboard_mapping(e.detail, 1)[0][0]
                print("KEY " + keysym_name(sym), flush=True)
            elif e.type == X.KeyRelease:
                sym = names.get_keyboard_mapping(e.detail, 1)[0][0]
                print("KEYUP " + keysym_name(sym), flush=True)
            elif e.type == X.ButtonPress:
                print("BUTTON %d" % e.detail, flush=True)
            elif e.type == X.ButtonRelease:
                print("BUTTONUP %d" % e.detail, flush=True)
            elif e.type == X.MotionNotify:
                print("MOTION %d %d" % (e.root_x, e.root_y), flush=True)

    threading.Timer(seconds, lambda: os._exit(0)).start()
    ctl.record_enable_context(ctx, seen)


if "--record" in sys.argv:
    record(float(sys.argv[sys.argv.index("--record") + 1]))

d = display.Display()
screen = d.screen()
# With --keys the window needs content: an X window without a background
# never gets a buffer, labwc never maps its view, and it never takes the
# keyboard focus. A --background colour wins; on the TrueColor visual the
# pixel value is 0xRRGGBB.
if background is None and keys is not None:
    background = screen.white_pixel
extra = {"background_pixel": background} if background is not None else {}
mask = X.StructureNotifyMask | (X.ButtonPressMask if buttons else 0)
win = screen.root.create_window(
    0, 0, 400, 300, 0, screen.root_depth,
    X.InputOutput, X.CopyFromParent,
    event_mask=mask, **extra)
win.set_wm_name("thinfloe-x11-probe")
win.set_wm_class("thinfloe-probe", "thinfloe-probe")
win.map()
d.sync()
time.sleep(1)

if grab:
    win.grab_keyboard(False, X.GrabModeAsync, X.GrabModeAsync, X.CurrentTime)
    d.sync()
    time.sleep(1)

XK.load_keysym_group("xf86")
if keymap:
    syms = []
    for name in keymap:
        sym = XK.string_to_keysym(name)
        if sym == X.NoSymbol and name.startswith("XF86"):
            sym = XK.string_to_keysym("XF86_" + name[4:])
        if sym == X.NoSymbol:
            sys.exit("x11-probe: unknown keysym " + name)
        syms.append([sym])
    d.change_keyboard_mapping(9, syms)
    d.sync()

def ask_fullscreen():
    net_wm_state = d.intern_atom("_NET_WM_STATE")
    fullscreen = d.intern_atom("_NET_WM_STATE_FULLSCREEN")
    # _NET_WM_STATE_ADD = 1, source = 1 (normal application)
    d.screen().root.send_event(
        event.ClientMessage(window=win, client_type=net_wm_state,
                            data=(32, [1, fullscreen, 0, 1, 0])),
        event_mask=X.SubstructureRedirectMask | X.SubstructureNotifyMask)
    d.sync()
    time.sleep(2)


if keys is not None:
    if fullscreen_keys:
        ask_fullscreen()
    win.change_attributes(event_mask=X.StructureNotifyMask | X.KeyPressMask)
    d.sync()
    end = time.monotonic() + keys
    while time.monotonic() < end:
        while d.pending_events():
            e = d.next_event()
            # Re-read the map after a change, e.g. the one --keymap made.
            if e.type == X.MappingNotify:
                d.refresh_keyboard_mapping(e)
            elif e.type == X.KeyPress:
                print("KEY " + keysym_name(d.keycode_to_keysym(e.detail, 0)),
                      flush=True)
        time.sleep(0.05)
    sys.exit(0)

ask_fullscreen()

geom = win.get_geometry()
print("GEOMETRY %dx%d" % (geom.width, geom.height), flush=True)

end = time.monotonic() + hold
while time.monotonic() < end:
    while d.pending_events():
        e = d.next_event()
        if e.type == X.ButtonPress:
            print("BUTTON %d %d" % (e.event_x, e.event_y), flush=True)
    time.sleep(0.05)
