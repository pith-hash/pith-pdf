<p align="center">
  <img src="https://pith-pdf.n24q02m.com/logo.svg" alt="pith-pdf" width="120">
</p>

<h1 align="center">pith-pdf</h1>

<p align="center">
  <strong>PDF text extraction across classic xref, xref streams, cmap and CID fonts</strong>
</p>

<p align="center">
  <a href="https://github.com/pith-hash/pith-pdf/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/pith-hash/pith-pdf/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://github.com/pith-hash/pith-pdf/actions/workflows/cd.yml"><img alt="CD" src="https://github.com/pith-hash/pith-pdf/actions/workflows/cd.yml/badge.svg"></a>
  <a href="https://github.com/pith-hash/pith-pdf/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/pith-hash/pith-pdf?display_name=tag&sort=semver"></a>
  <a href="https://github.com/n24q02m/better-semantic-release"><img alt="semantic-release" src="https://img.shields.io/badge/semantic--release-e10079?logo=semantic-release&logoColor=white"></a>
  <a href="LICENSE"><img alt="License: MIT" src="https://img.shields.io/github/license/pith-hash/pith-pdf"></a>
</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#quick-start">Quick start</a> ·
  <a href="#the-pith-suite-contract">Suite contract</a>
</p>

<!-- BEGIN: AUTO-GENERATED-CROSS-PROMO -->
<!-- END: AUTO-GENERATED-CROSS-PROMO -->

## What it does

Opens a PDF, resolves objects through the cross-reference (classic tables,
xref **streams** with `/W` field widths and `/Index` runs, `/Prev`
incremental-update chains, and `/ObjStm` object streams), walks the page
tree and extracts text from content streams:

- text operators `Tj`, `TJ`, `'`, `"` plus the positioning ops `Td`, `TD`,
  `Tm`, `T*`; `Do` recurses into Form XObjects;
- font decoding: `/ToUnicode` CMaps (`bfchar`, `bfrange` incl. array
  destinations and surrogate pairs), the five predefined encodings plus
  `/Differences` (AGL names + `uniXXXX`/`uXXXXXX`), and CID-keyed fonts
  (`/Type0`, `/Identity-H/-V` and embedded CMap streams);
- stream filters `FlateDecode` (zlib + raw-deflate fallback), `ASCII85`,
  `ASCIIHex` with `/DecodeParms` PNG/TIFF predictors.

Refusals, never guesses: encrypted documents refuse per page with object
context, CID fonts without `/ToUnicode` refuse, unsupported stream filters
refuse naming the filter, and a corrupt xref is rebuilt by scanning when
possible (reported via `Document::xref_was_rebuilt`). Every page failure
carries the page number; every object-level refusal carries the object.

## The pith suite contract

pith-pdf is part of the **pith** suite (pith-hash). Every suite repository
follows the same rules; CI enforces them mechanically:

- **Naming**: a library is always `pith-<domain>` (`pith-image`, `pith-audio`,
  `pith-zip`, ...). The curator/repository of repositories is the bare
  `pith-hash`. Never invent a second naming scheme inside the suite.
- **Version pinning**: cross-library dependencies pin `~0.1` (e.g.
  `pith-image = { version = "~0.1", path = "../pith-image" }`). The whole suite
  moves together inside 0.1.x; breaking changes require a suite-wide version
  bump, never a silent minor drift.
- **Zero third-party dependencies**: every crate depends only on other
  `pith-*` crates plus `std`. `scripts/check-zero-deps.py` (run in CI) fails
  the build on any other crate, for normal, build and dev dependencies alike.
- **No unsafe**: every crate root carries `#![forbid(unsafe_code)]`.
- **Hex-exact vectors**: `reference.json` at the repo root is the
  cross-language source of truth. The `gen-reference` binary regenerates it;
  CI verifies the committed copy is current (`gen-reference verify`), and CD
  ships the regenerated file with every SDK artifact. Python, Node and Go SDKs
  MUST test against the same bytes.

## Repository layout

```
src/                the pith-pdf library (no_std + alloc)
tests/fixtures      generated PDF corpus with expected extractions (PROVENANCE.md)
tools/gen-reference the vector generator binary (bin name: gen-reference)
reference.json      hex-exact cross-SDK test vectors
```

## Install

Rust (the core library):

```bash
cargo add pith-pdf
```

Python / Node / Go SDKs are published from the same cdylib on every release;
see the release assets or the package registries for the matching version.

## Quick start

```rust
let pdf = std::fs::read("document.pdf")?;
let text = pith_pdf::extract_text(&pdf)?;
// pages are joined by \x0c (form feed)
for (i, page) in text.split('\x0c').enumerate() {
    println!("--- page {} ---\n{}", i + 1, page);
}
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## Security

See [SECURITY.md](SECURITY.md).

## License

[MIT](LICENSE) © pith-hash
