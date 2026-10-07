// SPDX-License-Identifier: MIT
// Copyright (c) 2026 pith-hash
"use strict";

/**
 * pith-pdf SDK: PDF text extraction through koffi.
 *
 * The single Rust core (the `pith-pdf` cdylib built by
 * `cargo build --release`) is loaded at runtime; koffi is the only
 * runtime dependency.
 *
 * Discovery order (the suite's cdylib convention):
 *
 *  1. `PITH_CDYLIB` — an explicit cdylib *file* path;
 *  2. `PITH_CDYLIB_DIR` — a *directory* scanned for the cdylib names
 *     (the CD pipeline points this at `target/release`);
 *  3. `prebuilds/` — the packaged npm layout the CD publish job
 *     assembles, flat and per `<os-arch>` (e.g. `linux-x64`);
 *  4. `<repo root>/target/release` — the repository working-tree
 *     layout, so a source checkout runs against a local cargo build
 *     with no configuration.
 *
 * The FFI surface is one extraction operation plus one free:
 * `pith_pdf_extract_canonical` opens a whole PDF document and extracts
 * all text into the canonical byte stream the `reference.json` vectors
 * are defined over (page count big-endian, rebuilt flag, text length
 * big-endian, UTF-8 text), and `pith_pdf_free` releases the handed-out
 * buffer.
 */

const koffi = require("koffi");
const fs = require("node:fs");
const path = require("node:path");

const STATUS_OK = 0;
const STATUS_INVALID = -1;
const STATUS_REJECTED = -2;
/** An encrypted document, or an unimplemented format feature in the error chain. */
const STATUS_UNSUPPORTED = -3;

/** Every cdylib file name cargo may drop into the build directory, per platform. */
const CDYLIB_NAMES = ["pith_pdf.dll", "libpith_pdf.so", "libpith_pdf.dylib"];

const PKG_ROOT = path.join(__dirname);
const REPO_ROOT = path.resolve(__dirname, "..", "..");

/** FfiError: a non-zero status code came back from the cdylib. */
class FfiError extends Error {
  /**
   * @param {string} op the FFI operation name
   * @param {number} status the raw status code
   */
  constructor(op, status) {
    const kind =
      {
        [STATUS_INVALID]: "invalid argument",
        [STATUS_REJECTED]: "input rejected",
        [STATUS_UNSUPPORTED]: "unsupported feature",
      }[status] ?? "unknown failure";
    super(`${op} failed: ${kind} (status ${status})`);
    this.name = "FfiError";
    /** The raw status code the FFI returned. */
    this.status = status;
  }
}

/**
 * Locates the cdylib through the suite's discovery chain.
 * @returns {string} an absolute path to the cdylib file
 * @throws {Error} when nothing is found
 */
function findCdylib() {
  const explicit = process.env.PITH_CDYLIB;
  if (explicit && fs.statSync(explicit, { throwIfNoEntry: false })?.isFile()) {
    return path.resolve(explicit);
  }
  /** @type {string[]} */
  const dirs = [];
  const envDir = process.env.PITH_CDYLIB_DIR;
  if (envDir) {
    dirs.push(envDir);
    if (!path.isAbsolute(envDir)) {
      dirs.push(path.join(REPO_ROOT, envDir));
    }
  }
  const osArch = `${process.platform}-${process.arch}`;
  dirs.push(path.join(PKG_ROOT, "prebuilds", osArch));
  dirs.push(path.join(PKG_ROOT, "prebuilds"));
  dirs.push(path.join(REPO_ROOT, "target", "release"));
  for (const dir of dirs) {
    for (const name of CDYLIB_NAMES) {
      const p = path.join(dir, name);
      if (fs.statSync(p, { throwIfNoEntry: false })?.isFile()) return p;
    }
  }
  throw new Error(
    "no pith-pdf cdylib found (searched PITH_CDYLIB, PITH_CDYLIB_DIR, prebuilds/ and <repo>/target/release); " +
      "run `cargo build --release` first",
  );
}

let cached = undefined;

/**
 * Loads the cdylib and binds the exported symbols (lazily, once).
 * @returns {{extract: Function, free: Function}}
 */
function loadLibrary() {
  if (cached) return cached;
  const lib = koffi.load(findCdylib());
  const extract = lib.func("pith_pdf_extract_canonical", "int32_t", [
    "const uint8_t *",
    "size_t",
    koffi.out(koffi.pointer("void *")),
    koffi.out(koffi.pointer("size_t")),
  ]);
  const free = lib.func("void pith_pdf_free(void *ptr, size_t len)");
  cached = { extract, free };
  return cached;
}

/**
 * Opens a complete PDF document and extracts all text into the
 * canonical byte stream the `reference.json` vectors are defined over.
 * The handed-out cdylib buffer is copied into a JS Buffer and released
 * before returning.
 *
 * @param {Buffer} data the complete PDF file bytes
 * @returns {Buffer} the canonical stream (13-byte header + UTF-8 text)
 * @throws {FfiError} with `status === -2` for any malformed input and
 *   `status === -3` for an encrypted document or another unsupported
 *   format feature
 */
function extractCanonical(data) {
  if (!Buffer.isBuffer(data)) {
    throw new TypeError("data must be a Buffer");
  }
  const { extract, free } = loadLibrary();
  const out = [null];
  const outLen = [0];
  const status = extract(data, data.length, out, outLen);
  if (status !== STATUS_OK) {
    throw new FfiError("pith_pdf_extract_canonical", status);
  }
  try {
    // koffi.decode hands back a Uint8Array view over the external
    // buffer; copy it into a Buffer before the cdylib buffer is freed.
    return Buffer.from(koffi.decode(out[0], "uint8_t", Number(outLen[0])));
  } finally {
    free(out[0], Number(outLen[0]));
  }
}

/**
 * Re-expresses the canonical byte stream as a plain object.
 *
 * @param {Buffer} raw the canonical stream
 * @returns {{pages: number, rebuilt: boolean, text: string,
 *   textBytes: Buffer, raw: Buffer}}
 */
function parseCanonical(raw) {
  if (!Buffer.isBuffer(raw) || raw.length < 13) {
    throw new TypeError("canonical stream is shorter than the 13-byte header");
  }
  const textLen = Number(raw.readBigUInt64BE(5));
  if (raw.length < 13 + textLen) {
    throw new TypeError("canonical stream is shorter than its declared text length");
  }
  return {
    pages: raw.readUInt32BE(0),
    rebuilt: raw[4] === 1,
    text: raw.subarray(13, 13 + textLen).toString("utf8"),
    textBytes: raw.subarray(13, 13 + textLen),
    raw,
  };
}

module.exports = {
  STATUS_OK,
  STATUS_INVALID,
  STATUS_REJECTED,
  STATUS_UNSUPPORTED,
  CDYLIB_NAMES,
  FfiError,
  findCdylib,
  extractCanonical,
  parseCanonical,
};
