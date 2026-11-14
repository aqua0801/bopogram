"""Keyboard layouts: keystrokes -> bopomofo syllables.

The engine only ever sees syllables. Supporting another layout means adding
a module here; nothing in the engine changes.
"""

from .dachen import DACHEN
from .base import Layout, LayoutError

LAYOUTS: dict[str, Layout] = {"dachen": DACHEN}

__all__ = ["LAYOUTS", "Layout", "LayoutError", "DACHEN"]
