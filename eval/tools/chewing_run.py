"""Type the keystroke file into libchewing (the Chewing core) and save what it commits.

Usage (WSL, needs the libchewing3 and libchewing3-data packages):
    python3 tools/chewing_run.py KEYS_TSV OUT_TXT [--lib libchewing.so.3]

KEYS_TSV is the output of tools/ms_keys.py. Every sentence is typed into a
fresh context (default Dachen layout, Space as the first tone, longest
composition buffer, no learning carried between sentences) and committed with
Enter; one output line per sentence, in the same order, like the text typed
with tools/ms_type.py. Uses only the documented public C API, through ctypes.
"""

from __future__ import annotations

import argparse
import ctypes
import tempfile
from pathlib import Path

MAX_CHI_SYMBOL_LEN = 39


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("keys_tsv", type=Path)
    ap.add_argument("out", type=Path)
    ap.add_argument("--lib", default="libchewing.so.3")
    args = ap.parse_args()

    lib = ctypes.CDLL(args.lib)
    lib.chewing_new2.restype = ctypes.c_void_p
    lib.chewing_new2.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_void_p, ctypes.c_void_p]
    lib.chewing_commit_String.restype = ctypes.c_void_p
    for name in ("chewing_delete", "chewing_handle_Enter", "chewing_handle_Space",
                 "chewing_commit_Check", "chewing_buffer_Len", "chewing_zuin_Check"):
        getattr(lib, name).argtypes = [ctypes.c_void_p]
    lib.chewing_handle_Default.argtypes = [ctypes.c_void_p, ctypes.c_int]
    lib.chewing_set_maxChiSymbolLen.argtypes = [ctypes.c_void_p, ctypes.c_int]
    lib.chewing_set_spaceAsSelection.argtypes = [ctypes.c_void_p, ctypes.c_int]
    lib.chewing_set_KBType.argtypes = [ctypes.c_void_p, ctypes.c_int]
    lib.chewing_free.argtypes = [ctypes.c_void_p]

    def committed(ctx) -> str:
        if not lib.chewing_commit_Check(ctx):
            return ""
        ptr = lib.chewing_commit_String(ctx)
        text = ctypes.string_at(ptr).decode("utf-8")
        lib.chewing_free(ptr)
        return text

    with open(args.keys_tsv, encoding="utf-8") as fh:
        rows = [line.rstrip("\n").split("\t") for line in fh][1:]
    with tempfile.TemporaryDirectory() as tmp:  # user phrases go here and are thrown away
        out = []
        for _sid, keys in rows:
            ctx = lib.chewing_new2(None, str(Path(tmp) / "user.dat").encode(), None, None)
            if not ctx:
                raise SystemExit("chewing_new2 failed (is libchewing3-data installed?)")
            lib.chewing_set_KBType(ctx, 0)  # default (Dachen) layout
            lib.chewing_set_maxChiSymbolLen(ctx, MAX_CHI_SYMBOL_LEN)
            lib.chewing_set_spaceAsSelection(ctx, 0)
            text = ""
            for ch in keys:
                if ch == " ":
                    lib.chewing_handle_Space(ctx)
                else:
                    lib.chewing_handle_Default(ctx, ord(ch))
                text += committed(ctx)
            lib.chewing_handle_Enter(ctx)
            text += committed(ctx)
            out.append(text)
            lib.chewing_delete(ctx)
            (Path(tmp) / "user.dat").unlink(missing_ok=True)
    args.out.write_text("\n".join(out) + "\n", encoding="utf-8")
    print(f"{len(out)} sentences -> {args.out}")


if __name__ == "__main__":
    main()
