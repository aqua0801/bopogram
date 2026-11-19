"""Type keystroke lines into the focused window, to test another IME (Windows only).

Usage (Windows, plain Python 3, no packages needed):
    python ms_type.py KEYS_TSV [--start ID] [--key-delay 0.02] [--line-delay 0.3]

1. Open Notepad, switch to the IME under test (e.g. Microsoft Bopomofo,
   Chinese mode, Dachen layout) and click into the empty document.
2. Run this script from a terminal and click back into Notepad within the
   5-second countdown.
3. For every line of KEYS_TSV it presses the keys, then Enter (commit) and
   Enter again (new line). Afterwards save the document as UTF-8.

Safety: keystrokes go only to the window that had the focus when the
countdown ended. If the focus moves anywhere else (click another window to
stop), the script stops at once and prints the id to resume from with --start.
It only sends key presses; it changes no settings.
"""

from __future__ import annotations

import argparse
import ctypes
import time
from ctypes import wintypes

user32 = ctypes.WinDLL("user32", use_last_error=True)

INPUT_KEYBOARD = 1
KEYEVENTF_KEYUP = 0x0002
VK = {",": 0xBC, ".": 0xBE, "/": 0xBF, ";": 0xBA, "-": 0xBD, " ": 0x20, "\n": 0x0D}
VK.update({c: ord(c.upper()) for c in "abcdefghijklmnopqrstuvwxyz0123456789"})


class KEYBDINPUT(ctypes.Structure):
    _fields_ = [("wVk", wintypes.WORD), ("wScan", wintypes.WORD), ("dwFlags", wintypes.DWORD),
                ("time", wintypes.DWORD), ("dwExtraInfo", ctypes.c_size_t)]


class _U(ctypes.Union):
    _fields_ = [("ki", KEYBDINPUT), ("pad", ctypes.c_byte * 32)]


class INPUT(ctypes.Structure):
    _fields_ = [("type", wintypes.DWORD), ("u", _U)]


def press(ch: str) -> None:
    vk = VK[ch]
    scan = user32.MapVirtualKeyW(vk, 0)
    seq = (INPUT * 2)()
    for i, flags in enumerate((0, KEYEVENTF_KEYUP)):
        seq[i].type = INPUT_KEYBOARD
        seq[i].u.ki = KEYBDINPUT(vk, scan, flags, 0, 0)
    if user32.SendInput(2, seq, ctypes.sizeof(INPUT)) != 2:
        raise OSError(ctypes.get_last_error(), "SendInput failed")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("keys_tsv")
    ap.add_argument("--start", help="resume from this id")
    ap.add_argument("--key-delay", type=float, default=0.02)
    ap.add_argument("--line-delay", type=float, default=0.3)
    args = ap.parse_args()
    with open(args.keys_tsv, encoding="utf-8") as fh:
        rows = [line.rstrip("\n").split("\t") for line in fh][1:]
    if args.start:
        rows = rows[[r[0] for r in rows].index(args.start):]

    for s in range(5, 0, -1):
        print(f"click into the target window... {s}", flush=True)
        time.sleep(1)
    target = user32.GetForegroundWindow()
    print(f"typing {len(rows)} lines; click any other window to stop", flush=True)
    for n, (sid, keys) in enumerate(rows, 1):
        for ch in keys + "\n\n":
            if user32.GetForegroundWindow() != target:
                print(f"focus left the window: stopped before {sid} (resume with --start {sid})")
                return
            press(ch)
            time.sleep(args.key_delay)
        time.sleep(args.line_delay)
        if n % 50 == 0:
            print(f"{n}/{len(rows)}", flush=True)
    print("done: save the document as UTF-8")


if __name__ == "__main__":
    main()
