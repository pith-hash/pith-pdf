# SPDX-License-Identifier: MIT
# Copyright (c) 2026 pith-hash
"""Hex-exact conformance: the committed reference vectors through ctypes.

Every fixture vector in the repository-root ``reference.json`` is
replayed through the cdylib and compared byte-exact — the extracted
text's SHA-256 against ``text_sha256``, plus the page count, the
xref-rebuilt flag and the text byte length. The scan-recovery vector is
rebuilt from its inline hex document, the encrypted fixture must refuse
with status -3, and every ``Document::open`` refusal vector is replayed
with its exact status. The same vectors the Rust ``gen-reference
verify`` gate and the Node/Go SDKs check. The ``decode_stream`` error
rows are excluded by design: their ``input`` echo is a filter *name*,
not a replayable document.
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

import pytest

from pith_pdf import (
    STATUS_REJECTED,
    STATUS_UNSUPPORTED,
    FfiError,
    extract_canonical,
    find_cdylib,
    parse_canonical,
)

REPO_ROOT = Path(__file__).resolve().parents[3]
REFERENCE = json.loads((REPO_ROOT / "reference.json").read_bytes().decode("utf-8"))


def fixture_vectors() -> list[dict]:
    """The positive vectors driven by ``tests/fixtures/<name>.pdf``
    (the recovery row also carries a digest but is driven by its
    inline hex ``input``, not by a fixture file)."""
    return [v for v in REFERENCE["vectors"] if "text_sha256" in v and "input" not in v]


def test_cdylib_is_discoverable() -> None:
    path = find_cdylib()
    assert path.is_file(), path


@pytest.mark.parametrize(
    "vector", fixture_vectors(), ids=lambda v: v["name"]
)
def test_reference_vector_is_reproduced_hex_exact(vector: dict) -> None:
    data = (REPO_ROOT / "tests" / "fixtures" / f"{vector['name']}.pdf").read_bytes()

    raw = extract_canonical(data)
    canonical = parse_canonical(raw)

    assert hashlib.sha256(canonical.text_bytes).hexdigest() == vector["text_sha256"], vector["name"]
    assert len(canonical.text_bytes) == vector["text_bytes"], vector["name"]
    assert canonical.pages == vector["pages"], vector["name"]
    assert canonical.rebuilt == vector["rebuilt"], vector["name"]


def test_recovered_startxref_vector_is_rebuilt_from_inline_hex() -> None:
    vector = next(v for v in REFERENCE["vectors"] if v["name"] == "recovered_startxref")
    raw = extract_canonical(bytes.fromhex(vector["input"]))
    canonical = parse_canonical(raw)
    assert hashlib.sha256(canonical.text_bytes).hexdigest() == vector["text_sha256"]
    assert len(canonical.text_bytes) == vector["text_bytes"]
    assert canonical.pages == vector["pages"]
    assert canonical.rebuilt is True


def test_encrypted_fixture_refuses_unsupported() -> None:
    data = (REPO_ROOT / "tests" / "fixtures" / "encrypted.pdf").read_bytes()
    with pytest.raises(FfiError) as err:
        extract_canonical(data)
    assert err.value.status == STATUS_UNSUPPORTED


@pytest.mark.parametrize(
    "row",
    [e for e in REFERENCE["errors"] if e["api"] == "Document::open"],
    ids=lambda e: e["name"],
)
def test_open_refusal_vector_is_replayed(row: dict) -> None:
    with pytest.raises(FfiError) as err:
        extract_canonical(bytes.fromhex(row["input"]))
    expected = STATUS_UNSUPPORTED if "unsupported" in row["error"] else STATUS_REJECTED
    assert err.value.status == expected, row["name"]


def test_decode_stream_error_rows_are_excluded_by_design() -> None:
    excluded = [e for e in REFERENCE["errors"] if e["api"] != "Document::open"]
    replayed = [e for e in REFERENCE["errors"] if e["api"] == "Document::open"]
    assert len(excluded) + len(replayed) == len(REFERENCE["errors"])
    assert all(e["api"] == "pith_pdf::decode_stream" for e in excluded)
    # Their `input` echo is a filter name, not a document: replaying them
    # would test nothing. The count is pinned so a reference.json edit
    # that silently re-classifies a row fails loudly here.
    assert len(excluded) == 11
    assert len(replayed) == 5


def test_malformed_input_is_refused_not_crashing() -> None:
    with pytest.raises(FfiError) as err:
        extract_canonical(b"not a pdf at all")
    assert err.value.status == STATUS_REJECTED


def test_empty_input_is_refused() -> None:
    with pytest.raises(FfiError):
        extract_canonical(b"")


def test_full_stream_matches_a_rust_pinned_value() -> None:
    # empty_page's canonical-stream digest, pinned in the Rust unit
    # tests and re-derived there; this test fails loudly even if
    # reference.json were regenerated wrongly.
    data = (REPO_ROOT / "tests" / "fixtures" / "empty_page.pdf").read_bytes()
    raw = extract_canonical(data)
    assert hashlib.sha256(raw).hexdigest() == "5a8db37861d2ffa061f87ad2ec1b4c55181e695473ec20dbd228bad355b40e7e"
    assert list(raw[:13]) == [0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 14]
