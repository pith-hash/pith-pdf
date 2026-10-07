# SPDX-License-Identifier: MIT
# Copyright (c) 2026 pith-hash
"""pith-pdf SDK: PDF text extraction through ctypes.

The single Rust core (the ``pith-pdf`` cdylib built by
``cargo build --release``) is loaded at runtime; this package carries
no third-party dependency — ``ctypes`` is the standard library.

Discovery order (the suite's cdylib convention):

1. ``PITH_CDYLIB`` — an explicit cdylib *file* path;
2. ``PITH_CDYLIB_DIR`` — a *directory* scanned for the cdylib names
   (the CD pipeline points this at ``target/release``);
3. the package directory itself (the built wheel ships the cdylib as
   package data);
4. ``<repo root>/target/release`` — the repository working-tree layout,
   so a source checkout runs against a local cargo build with no
   configuration.

The FFI surface is one extraction operation plus one free:
``pith_pdf_extract_canonical`` opens a whole PDF document and extracts
all text into the canonical byte stream the ``reference.json`` vectors
are defined over (page count big-endian, rebuilt flag, text length
big-endian, UTF-8 text), and ``pith_pdf_free`` releases the handed-out
buffer.
"""

from __future__ import annotations

import ctypes
import os
from dataclasses import dataclass
from pathlib import Path

__all__ = [
    "Canonical",
    "FfiError",
    "LibraryNotFoundError",
    "find_cdylib",
    "extract_canonical",
    "parse_canonical",
    "STATUS_OK",
    "STATUS_INVALID",
    "STATUS_REJECTED",
    "STATUS_UNSUPPORTED",
]

#: Status: success.
STATUS_OK = 0
#: Status: a caller argument is invalid (a null pointer).
STATUS_INVALID = -1
#: Status: the core refused the input (malformed PDF).
STATUS_REJECTED = -2
#: Status: the core refuses to guess — an encrypted document, or an
#: unimplemented format feature (stream filter, CID font without
#: `/ToUnicode`) anywhere in the error chain.
STATUS_UNSUPPORTED = -3

#: Every cdylib file name cargo may drop into the build directory, per
#: platform (windows / linux / macOS).
CDYLIB_NAMES = ("pith_pdf.dll", "libpith_pdf.so", "libpith_pdf.dylib")


@dataclass(frozen=True)
class Canonical:
    """The extracted document, re-expressed from the canonical byte stream.

    ``text`` is every page's text joined with ``\\x0c`` (form feed) —
    exactly the bytes the vectors' ``text_sha256`` digest covers.
    """

    #: Page count in document order.
    pages: int
    #: Whether the xref was rebuilt by scanning (a recovery ran).
    rebuilt: bool
    #: The extracted text, pages joined with ``\\x0c``.
    text: str
    #: The canonical byte stream the digest is computed over.
    raw: bytes

    @property
    def text_bytes(self) -> bytes:
        """The extracted text as UTF-8 (everything after the 13-byte header)."""
        return self.raw[13:]


class LibraryNotFoundError(OSError):
    """No cdylib was found through the discovery chain."""


class FfiError(Exception):
    """A non-zero status code came back from the cdylib."""

    def __init__(self, op: str, status: int) -> None:
        kind = {
            STATUS_INVALID: "invalid argument",
            STATUS_REJECTED: "input rejected",
            STATUS_UNSUPPORTED: "unsupported feature",
        }.get(status, "unknown failure")
        super().__init__(f"{op} failed: {kind} (status {status})")
        #: The raw status code the FFI returned.
        self.status = status


def find_cdylib() -> Path:
    """Locates the cdylib through the suite's discovery chain."""
    explicit = os.environ.get("PITH_CDYLIB")
    if explicit:
        p = Path(explicit)
        if p.is_file():
            return p
    env_dir = os.environ.get("PITH_CDYLIB_DIR")
    candidates: list[Path] = []
    if env_dir:
        env_dir_path = Path(env_dir)
        candidates.append(env_dir_path)
        if not env_dir_path.is_absolute():
            # CD and local runs invoke tools from the repository root or
            # from sdk/<lang>; resolve the env value against both.
            candidates.append(Path.cwd() / env_dir_path)
            candidates.append(Path(__file__).resolve().parents[3] / env_dir_path)
    candidates.append(Path(__file__).resolve().parent)  # packaged wheel
    candidates.append(Path(__file__).resolve().parents[3] / "target" / "release")
    for directory in candidates:
        for name in CDYLIB_NAMES:
            p = directory / name
            if p.is_file():
                return p
    raise LibraryNotFoundError(
        "no pith-pdf cdylib found (searched PITH_CDYLIB, PITH_CDYLIB_DIR, "
        "the package directory and <repo>/target/release); "
        "run `cargo build --release` first"
    )


_lib: ctypes.CDLL | None = None


def _load() -> ctypes.CDLL:
    global _lib
    if _lib is None:
        lib = ctypes.CDLL(str(find_cdylib()))
        lib.pith_pdf_extract_canonical.argtypes = [
            ctypes.c_void_p,  # data
            ctypes.c_size_t,  # len
            ctypes.POINTER(ctypes.c_void_p),  # out buffer
            ctypes.POINTER(ctypes.c_size_t),  # out length
        ]
        lib.pith_pdf_extract_canonical.restype = ctypes.c_int32
        lib.pith_pdf_free.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
        lib.pith_pdf_free.restype = None
        _lib = lib
    return _lib


def extract_canonical(data: bytes) -> bytes:
    """Opens a complete PDF document and extracts all text into the
    canonical byte stream the ``reference.json`` vectors are defined
    over.

    Raises :class:`FfiError` with ``status == STATUS_REJECTED`` for any
    malformed input (truncated header, unreadable xref, parse error)
    and ``status == STATUS_UNSUPPORTED`` for an encrypted document or
    another unimplemented format feature; the extractor never panics
    through this boundary.
    """
    out = ctypes.c_void_p()
    out_len = ctypes.c_size_t()
    status = _load().pith_pdf_extract_canonical(data, len(data), ctypes.byref(out), ctypes.byref(out_len))
    if status != STATUS_OK:
        raise FfiError("pith_pdf_extract_canonical", status)
    try:
        return ctypes.string_at(out, out_len.value)
    finally:
        _load().pith_pdf_free(out, out_len.value)


def parse_canonical(raw: bytes) -> Canonical:
    """Re-expresses the canonical byte stream as a :class:`Canonical`."""
    if len(raw) < 13:
        raise ValueError("canonical stream is shorter than the 13-byte header")
    text_len = int.from_bytes(raw[5:13], "big")
    if len(raw) < 13 + text_len:
        raise ValueError("canonical stream is shorter than its declared text length")
    return Canonical(
        pages=int.from_bytes(raw[0:4], "big"),
        rebuilt=raw[4] == 1,
        text=raw[13 : 13 + text_len].decode("utf-8"),
        raw=raw,
    )
