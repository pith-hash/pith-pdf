// SPDX-License-Identifier: MIT
// Copyright (c) 2026 pith-hash
"use strict";

// Hex-exact conformance: the committed reference vectors through koffi.
// Every fixture vector in the repository-root reference.json is replayed
// through the cdylib and compared byte-exact — the extracted text's
// SHA-256 against text_sha256, plus the page count, the xref-rebuilt
// flag and the text byte length. The scan-recovery vector is rebuilt
// from its inline hex document, the encrypted fixture must refuse with
// status -3, and every Document::open refusal vector is replayed with
// its exact status. The decode_stream error rows are excluded by
// design: their input echo is a filter *name*, not a replayable
// document. The same vectors the Rust gen-reference verify gate and
// the Python/Go SDKs check.

const test = require("node:test");
const assert = require("node:assert/strict");
const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");

const {
  STATUS_REJECTED,
  STATUS_UNSUPPORTED,
  FfiError,
  extractCanonical,
  findCdylib,
  parseCanonical,
} = require("../index.js");

const REPO_ROOT = path.resolve(__dirname, "..", "..", "..");

const REFERENCE = JSON.parse(fs.readFileSync(path.join(REPO_ROOT, "reference.json"), "utf8"));
const FIXTURE_VECTORS = REFERENCE.vectors.filter(
  (v) => "text_sha256" in v && !("input" in v),
);
const OPEN_REFUSALS = REFERENCE.errors.filter((e) => e.api === "Document::open");
const EXCLUDED_REFUSALS = REFERENCE.errors.filter((e) => e.api !== "Document::open");

test("cdylib is discoverable", () => {
  assert.ok(fs.statSync(findCdylib()).isFile());
});

for (const vector of FIXTURE_VECTORS) {
  test(`reference vector ${vector.name} is reproduced hex-exact`, () => {
    const data = fs.readFileSync(path.join(REPO_ROOT, "tests", "fixtures", `${vector.name}.pdf`));

    const canonical = parseCanonical(extractCanonical(data));
    assert.equal(
      crypto.createHash("sha256").update(canonical.textBytes).digest("hex"),
      vector.text_sha256,
      vector.name,
    );
    assert.equal(canonical.textBytes.length, vector.text_bytes, vector.name);
    assert.equal(canonical.pages, vector.pages, vector.name);
    assert.equal(canonical.rebuilt, vector.rebuilt, vector.name);
  });
}

test("reference vector recovered_startxref is rebuilt from inline hex", () => {
  const vector = REFERENCE.vectors.find((v) => v.name === "recovered_startxref");
  const canonical = parseCanonical(extractCanonical(Buffer.from(vector.input, "hex")));
  assert.equal(
    crypto.createHash("sha256").update(canonical.textBytes).digest("hex"),
    vector.text_sha256,
  );
  assert.equal(canonical.textBytes.length, vector.text_bytes);
  assert.equal(canonical.pages, vector.pages);
  assert.equal(canonical.rebuilt, true);
});

test("encrypted fixture refuses unsupported", () => {
  const data = fs.readFileSync(path.join(REPO_ROOT, "tests", "fixtures", "encrypted.pdf"));
  assert.throws(() => extractCanonical(data), (err) => {
    assert.ok(err instanceof FfiError);
    assert.equal(err.status, STATUS_UNSUPPORTED);
    return true;
  });
});

for (const row of OPEN_REFUSALS) {
  test(`refusal vector ${row.name} is replayed`, () => {
    assert.throws(() => extractCanonical(Buffer.from(row.input, "hex")), (err) => {
      assert.ok(err instanceof FfiError);
      const expected = row.error.includes("unsupported") ? STATUS_UNSUPPORTED : STATUS_REJECTED;
      assert.equal(err.status, expected, row.name);
      return true;
    });
  });
}

test("decode_stream error rows are excluded by design", () => {
  assert.equal(EXCLUDED_REFUSALS.length + OPEN_REFUSALS.length, REFERENCE.errors.length);
  assert.ok(EXCLUDED_REFUSALS.every((e) => e.api === "pith_pdf::decode_stream"));
  // Their `input` echo is a filter name, not a document: replaying them
  // would test nothing. The counts are pinned so a reference.json edit
  // that silently re-classifies a row fails loudly here.
  assert.equal(EXCLUDED_REFUSALS.length, 11);
  assert.equal(OPEN_REFUSALS.length, 5);
});

test("malformed input is refused, not crashing", () => {
  assert.throws(() => extractCanonical(Buffer.from("not a pdf at all")), (err) => {
    assert.ok(err instanceof FfiError);
    assert.equal(err.status, STATUS_REJECTED);
    return true;
  });
});

test("empty input is refused", () => {
  assert.throws(() => extractCanonical(Buffer.alloc(0)), FfiError);
});

test("full stream matches a rust-pinned value", () => {
  // empty_page's canonical-stream digest, pinned in the Rust unit
  // tests and re-derived there; this test fails loudly even if
  // reference.json were regenerated wrongly.
  const data = fs.readFileSync(path.join(REPO_ROOT, "tests", "fixtures", "empty_page.pdf"));
  const raw = extractCanonical(data);
  assert.equal(
    crypto.createHash("sha256").update(raw).digest("hex"),
    "5a8db37861d2ffa061f87ad2ec1b4c55181e695473ec20dbd228bad355b40e7e",
  );
  assert.deepEqual([...raw.subarray(0, 13)], [0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 14]);
});
