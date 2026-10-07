//! Regenerates and verifies `reference.json`, the hex-exact PDF
//! reference this suite ships beside every SDK artifact.
//!
//! The corpus is the committed fixture set `tests/fixtures/*.pdf` with its
//! expected extractions in `tests/fixtures/*.txt` (provenance and the
//! generation oracles in `tests/fixtures/PROVENANCE.md`); this binary turns
//! that corpus into a language-neutral JSON document by *opening and
//! extracting every document through `pith-pdf`*, checking each extraction
//! against its `.txt` truth file, and pinning every page count and text
//! payload with its SHA-256 digest. Two further sections pin the
//! documented edge behaviour: the scan-rebuild recovery of a corrupt
//! `startxref`, and the refusal surface — malformed xref chains and
//! malformed stream objects, each with its exact input bytes and the
//! deterministic error message the suite must produce. The result is a
//! fixed point: `reference.json` is exactly what the current extractor
//! produces for the current fixtures and refusals, so any regression in
//! either shows up as a `verify` failure instead of a silently stale file.
//!
//! - `gen-reference gen` writes `reference.json` at the repository root.
//! - `gen-reference verify` recomputes it and fails on any drift; this is
//!   the mode the CI gate runs.
//!
//! The binary uses `std` (it touches the filesystem) but adds no
//! dependencies: the JSON emission is hand-rolled, and the digest is the
//! suite's own `pith_digest::sha256` — the same function downstream SDKs
//! use to check the pinned text payloads.

use std::fs;
use std::path::{Path, PathBuf};

use pith_digest::sha256;
use pith_inflate::Limits;
use pith_pdf::{Document, Obj, decode_stream};

/// Where the reference file lives: the repository root, next to the crate
/// manifest, so the CD workflow can ship it with the SDK artifacts
/// regardless of the directory `cargo run` was invoked from.
fn reference_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("reference.json")
}

/// Where the committed document fixtures live.
fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// One committed fixture: the `.pdf` file, the extraction truth this
/// reference must reproduce, and why the file is in the corpus.
struct Fixture {
    /// File name under `tests/fixtures/` (without extension).
    stem: &'static str,
    /// Why this fixture is here: the structure it exercises.
    why: &'static str,
    /// Encrypted fixtures refuse extraction per page; their vector pins
    /// the refusal message instead of a text digest.
    encrypted: bool,
}

/// The corpus, in deterministic order: `gen-reference` walks it in this
/// order and the emitted JSON keeps it, so a diff against a previous
/// commit reads as a changelog of the fixture set.
const FIXTURES: &[Fixture] = &[
    Fixture {
        stem: "empty_page",
        why: "a content-less page: its only extraction output is the page join",
        encrypted: false,
    },
    Fixture {
        stem: "standard_default",
        why: "no /Encoding: StandardEncoding default, unmapped byte decodes as U+FFFD",
        encrypted: false,
    },
    Fixture {
        stem: "standardenc",
        why: "explicit /StandardEncoding with escape-decodable names",
        encrypted: false,
    },
    Fixture {
        stem: "winansi_basic",
        why: "/WinAnsiEncoding one-byte codes",
        encrypted: false,
    },
    Fixture {
        stem: "winansi_tj_kern",
        why: "TJ kerning gaps below -250 split words",
        encrypted: false,
    },
    Fixture {
        stem: "macroman",
        why: "/MacRomanEncoding table incl. non-Latin-1 code points",
        encrypted: false,
    },
    Fixture {
        stem: "symbol",
        why: "/Symbol predefined encoding",
        encrypted: false,
    },
    Fixture {
        stem: "zapfdingbats",
        why: "/ZapfDingbats predefined encoding",
        encrypted: false,
    },
    Fixture {
        stem: "differences",
        why: "/Differences overrides on WinAnsi (AGL + uniXXXX names)",
        encrypted: false,
    },
    Fixture {
        stem: "tounicode_bfchar",
        why: "/ToUnicode CMap with bfchar entries",
        encrypted: false,
    },
    Fixture {
        stem: "tounicode_bfrange",
        why: "/ToUnicode CMap with bfrange scalar and array forms",
        encrypted: false,
    },
    Fixture {
        stem: "tounicode_surrogate",
        why: "/ToUnicode CMap with a surrogate-pair destination",
        encrypted: false,
    },
    Fixture {
        stem: "cid_identity",
        why: "/Type0 Identity-H CID font with ToUnicode",
        encrypted: false,
    },
    Fixture {
        stem: "cid_cmapstream",
        why: "/Type0 CID font with an embedded CMap stream",
        encrypted: false,
    },
    Fixture {
        stem: "xrefstream",
        why: "xref stream cross-reference (no classic table)",
        encrypted: false,
    },
    Fixture {
        stem: "xrefstream_w",
        why: "xref stream with non-default /W field widths",
        encrypted: false,
    },
    Fixture {
        stem: "objstm",
        why: "objects carried in /ObjStm object streams",
        encrypted: false,
    },
    Fixture {
        stem: "prev_incremental",
        why: "/Prev incremental update: newest section wins",
        encrypted: false,
    },
    Fixture {
        stem: "corrupt_xref_recoverable",
        why: "trashed xref rebuilt by scanning for object headers",
        encrypted: false,
    },
    Fixture {
        stem: "flate_content",
        why: "FlateDecode content stream",
        encrypted: false,
    },
    Fixture {
        stem: "contents_array",
        why: "content stream delivered as an array of streams",
        encrypted: false,
    },
    Fixture {
        stem: "formxobject",
        why: "text inside a Form XObject invoked by Do",
        encrypted: false,
    },
    Fixture {
        stem: "markcontent",
        why: "marked-content operators (BDC/EMC) pass through",
        encrypted: false,
    },
    Fixture {
        stem: "type3",
        why: "Type3 glyph procedures drawing text",
        encrypted: false,
    },
    Fixture {
        stem: "encrypted",
        why: "RC4 V1/R2 Standard security: extraction refuses with page and object context",
        encrypted: true,
    },
];

/// Reads one fixture document from `tests/fixtures/`. A missing or
/// unreadable fixture is a broken build, not a reference to write.
fn read_fixture(stem: &str) -> Vec<u8> {
    let path = fixtures_dir().join(format!("{stem}.pdf"));
    fs::read(&path).unwrap_or_else(|e| panic!("cannot read fixture {}: {e}", path.display()))
}

/// Reads one fixture's expected extraction. The `.txt` files are the
/// canonical truth the reference must agree with, never derived from the
/// extractor.
fn expected_text(stem: &str) -> String {
    let path = fixtures_dir().join(format!("{stem}.txt"));
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read fixture truth {}: {e}", path.display()))
}

/// Opens one fixture document. A fixture the reader refuses is a broken
/// build: the reference must never paper over a reader change.
fn open_fixture<'a>(stem: &str, data: &'a [u8]) -> Document<'a> {
    Document::open(data).unwrap_or_else(|e| panic!("fixture {stem} failed to open: {e}"))
}

/// Asserts one opened document matches its truth file and returns the
/// extraction facts the reference pins (page count, xref-rebuild flag,
/// text bytes).
fn extract_fixture(stem: &str, doc: &Document<'_>) -> (usize, bool, Vec<u8>) {
    let text = doc
        .text()
        .unwrap_or_else(|e| panic!("fixture {stem} failed to extract: {e}"));
    let expected = expected_text(stem);
    assert_eq!(
        text, expected,
        "fixture {stem} extraction drifted from its .txt truth file"
    );
    (doc.pages(), doc.xref_was_rebuilt(), text.into_bytes())
}

/// Lowercase hex of a byte slice (used for input echoes and SHA-256
/// digests).
fn hex(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 2);
    for byte in data {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Escapes one string for a JSON string literal. The corpus strings are
/// plain ASCII names and prose, but the escaper is complete so the output
/// can never be corrupted by a future edit to either.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// A valid zlib stream of `b"predictor"` (fixed-Huffman deflate): the
/// payload the FlateDecode+predictor refusal vectors decode successfully
/// before the DecodeParms pass refuses.
const ZLIB_PAYLOAD: &[u8] = &[
    0x78, 0x01, 0x2b, 0x28, 0x4a, 0x4d, 0xc9, 0x4c, 0x2e, 0xc9, 0x2f, 0x02, 0x00, 0x12, 0xe9, 0x03,
    0xcd,
];

/// Name object, minus the leading slash.
fn name(n: &[u8]) -> Obj {
    Obj::Name(n.to_vec())
}

/// Integer-valued number object.
fn int(v: i64) -> Obj {
    Obj::Num(v as f64)
}

/// Dictionary from ordered key/value pairs.
fn dict(kvs: &[(&[u8], Obj)]) -> Obj {
    Obj::Dict(kvs.iter().map(|(k, v)| (k.to_vec(), v.clone())).collect())
}

/// Stream object from ordered dictionary pairs and raw bytes.
fn stream(kvs: &[(&[u8], Obj)], data: &[u8]) -> Obj {
    Obj::Stream {
        dict: kvs.iter().map(|(k, v)| (k.to_vec(), v.clone())).collect(),
        data: data.to_vec(),
    }
}

/// One vector row: name, why, API entry, input echo (what a foreign SDK
/// needs to rebuild the bytes), and the object the API is called with.
type ErrorRow = (&'static str, &'static str, &'static str, Vec<u8>, Obj);

/// Builds every refusal vector. The expected error for each is asserted
/// (not just emitted) against [`expected_error`], so a silently changed
/// message cannot ship as a reference.
fn error_vectors() -> Vec<ErrorRow> {
    let mut v: Vec<ErrorRow> = Vec::new();
    let open = |name: &'static str, why: &'static str, input: &[u8], out: &mut Vec<ErrorRow>| {
        out.push((name, why, "Document::open", input.to_vec(), Obj::Null));
    };
    let dec = |name: &'static str, why: &'static str, obj: Obj, out: &mut Vec<ErrorRow>| {
        // The input echo carries the filter name where one exists: that is
        // the only field a foreign SDK needs to rebuild the refusal.
        let echo = match obj.get(b"Filter").and_then(Obj::as_bytes) {
            Some(f) => f.to_vec(),
            None => b"none".to_vec(),
        };
        out.push((name, why, "pith_pdf::decode_stream", echo, obj));
    };

    open(
        "truncated_header",
        "a 7-byte buffer cannot even hold the %PDF- signature",
        b"%PDF-1.",
        &mut v,
    );
    open(
        "no_signature",
        "binary junk with no %PDF- marker inside the first KiB",
        b"not a pdf at all, just junk bytes....",
        &mut v,
    );
    open(
        "missing_startxref",
        "well-formed header but no startxref marker and no scannable objects",
        b"%PDF-1.4\nnothing here but prose\n",
        &mut v,
    );
    open(
        "bad_startxref_value",
        "startxref marker present but its offset is not a number, and nothing is scannable",
        b"%PDF-1.4\nstartxref\nzz\n%%EOF\n",
        &mut v,
    );
    open(
        "startxref_past_eof",
        "startxref points past the end of the file and nothing is scannable",
        b"%PDF-1.4\nstartxref\n999\n%%EOF\n",
        &mut v,
    );

    let lzw = stream(&[(b"Filter", name(b"LZWDecode"))], b"");
    dec(
        "lzw_filter_refused",
        "LZWDecode is a real filter this suite refuses to guess",
        lzw,
        &mut v,
    );
    let crypt = stream(&[(b"Filter", name(b"Crypt"))], b"");
    dec(
        "crypt_filter_refused",
        "Crypt streams cannot be decoded without the security handler",
        crypt,
        &mut v,
    );
    let dct = stream(&[(b"Filter", name(b"DCTDecode"))], b"");
    dec(
        "dct_filter_refused",
        "DCTDecode (JPEG) is out of scope and named, never silently skipped",
        dct,
        &mut v,
    );
    let filter_el = stream(&[(b"Filter", Obj::Arr(vec![int(0)]))], b"");
    dec(
        "filter_element_type",
        "a /Filter array element that is not a name is a structural error",
        filter_el,
        &mut v,
    );
    let filter_ty = stream(&[(b"Filter", int(0))], b"");
    dec(
        "filter_type",
        "a /Filter that is neither name nor array is a structural error",
        filter_ty,
        &mut v,
    );
    let parms_ty = stream(&[(b"DecodeParms", int(0))], b"");
    dec(
        "decodeparms_type",
        "a /DecodeParms that is neither dict nor array is a structural error",
        parms_ty,
        &mut v,
    );
    let ahx = stream(&[(b"Filter", name(b"ASCIIHexDecode"))], b"zz");
    dec(
        "asciihex_bad_char",
        "a non-hex character inside an ASCIIHexDecode stream",
        ahx,
        &mut v,
    );
    let a85 = stream(&[(b"Filter", name(b"ASCII85Decode"))], b"87cUR~");
    dec(
        "ascii85_bad_terminator",
        "an ASCII85Decode stream whose ~ is not followed by >",
        a85,
        &mut v,
    );
    let dims = stream(
        &[
            (b"Filter", name(b"FlateDecode")),
            (
                b"DecodeParms",
                dict(&[(b"Predictor", int(2)), (b"Colors", int(0))]),
            ),
        ],
        ZLIB_PAYLOAD,
    );
    dec(
        "predictor_dimensions",
        "a TIFF predictor with a non-positive /Colors dimension",
        dims,
        &mut v,
    );
    let bpc = stream(
        &[
            (b"Filter", name(b"FlateDecode")),
            (
                b"DecodeParms",
                dict(&[(b"Predictor", int(2)), (b"BitsPerComponent", int(4))]),
            ),
        ],
        ZLIB_PAYLOAD,
    );
    dec(
        "tiff_predictor_bpc",
        "the TIFF predictor is only implemented for 8 bits per component",
        bpc,
        &mut v,
    );
    let pv = stream(
        &[
            (b"Filter", name(b"FlateDecode")),
            (b"DecodeParms", dict(&[(b"Predictor", int(9))])),
        ],
        ZLIB_PAYLOAD,
    );
    dec(
        "predictor_value",
        "predictor 9 is neither TIFF (2) nor a PNG optimum (10-15)",
        pv,
        &mut v,
    );
    v
}

/// The exact error each vector must produce, spelled next to the input:
/// `gen` asserts the live error against this string before pinning it, so
/// a changed refusal message can never ship silently.
fn expected_error(name: &str) -> &'static str {
    match name {
        "truncated_header" => "truncated: PDF header",
        "no_signature" => "invalid magic: PDF signature",
        "missing_startxref" => "bad value: no objects found while scanning",
        "bad_startxref_value" => "bad value: no objects found while scanning",
        "startxref_past_eof" => "bad value: no objects found while scanning",
        "lzw_filter_refused" => "unsupported: LZWDecode stream filter",
        "crypt_filter_refused" => "unsupported: Crypt stream filter",
        "dct_filter_refused" => "unsupported: DCTDecode stream filter",
        "filter_element_type" => "bad value: /Filter element type",
        "filter_type" => "bad value: /Filter type",
        "decodeparms_type" => "bad value: /DecodeParms type",
        "asciihex_bad_char" => "bad value: ASCIIHex character",
        "ascii85_bad_terminator" => "bad value: ASCII85 terminator",
        "predictor_dimensions" => "bad value: DecodeParms dimensions",
        "tiff_predictor_bpc" => "unsupported: TIFF predictor bpc != 8",
        "predictor_value" => "bad value: Predictor value",
        other => unreachable!("undeclared error vector {other}"),
    }
}

/// Runs one refusal vector through its public API and returns the
/// observed message.
fn run_error_vector(api: &str, obj: &Obj, input: &[u8]) -> String {
    match api {
        "Document::open" => {
            let err = match Document::open(input) {
                Err(e) => e,
                Ok(_) => panic!("refusal vector must refuse"),
            };
            format!("{err}")
        }
        "pith_pdf::decode_stream" => {
            let err = match decode_stream(obj, &Limits::default()) {
                Err(e) => e,
                Ok(_) => panic!("refusal vector must refuse"),
            };
            format!("{err}")
        }
        other => unreachable!("undeclared error api {other}"),
    }
}

/// The recovery document: a valid page tree whose `startxref` offset is
/// garbage, so the extractor must rebuild the xref by scanning for
/// object headers.
fn recovery_document_bytes() -> Vec<u8> {
    b"%PDF-1.4\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n\
3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>\nendobj\n\
startxref\nnot-an-offset\n%%EOF\n"
        .to_vec()
}

/// The exact bytes of `reference.json` for the current fixtures, refusals
/// and reader: two-space indent, corpus order, one trailing newline.
/// Nothing here is sorted or deduplicated on purpose - the file is a
/// transcript of the corpus, in corpus order, so a diff against a previous
/// commit reads as a changelog of the fixture set.
fn reference_json() -> String {
    let vectors = error_vectors();
    let mut out = String::with_capacity(16 * 1024);
    out.push_str("{\n");
    out.push_str("  \"schema\": 1,\n");
    out.push_str("  \"crate\": \"pith-pdf\",\n");
    out.push_str(
        "  \"description\": \"hex-exact PDF text-extraction reference vectors: every \
tests/fixtures/*.pdf document opened and extracted by pith-pdf against its .txt \
truth file and pinned by page count and SHA-256 text digest, plus a scan-rebuild \
recovery vector and refusal vectors for malformed xref chains and stream objects \
with exact error messages\",\n",
    );

    out.push_str("  \"vectors\": [\n");
    for fixture in FIXTURES {
        let data = read_fixture(fixture.stem);
        let doc = open_fixture(fixture.stem, &data);
        out.push_str("    {\n");
        out.push_str(&format!(
            "      \"name\": \"{}\",\n",
            json_escape(fixture.stem)
        ));
        out.push_str(&format!(
            "      \"why\": \"{}\",\n",
            json_escape(fixture.why)
        ));
        if fixture.encrypted {
            let err = doc
                .text()
                .expect_err("the encrypted fixture must refuse extraction");
            out.push_str(&format!("      \"pages\": {},\n", doc.pages()));
            out.push_str("      \"encrypted\": true,\n");
            out.push_str(&format!(
                "      \"error\": \"{}\"\n",
                json_escape(&format!("{err}"))
            ));
        } else {
            let (pages, rebuilt, text) = extract_fixture(fixture.stem, &doc);
            let digest = sha256(&text)
                .unwrap_or_else(|e| panic!("fixture {} sha256 failed: {e}", fixture.stem));
            out.push_str(&format!("      \"pages\": {pages},\n"));
            out.push_str(&format!("      \"rebuilt\": {rebuilt},\n"));
            out.push_str(&format!("      \"text_bytes\": {},\n", text.len()));
            out.push_str(&format!(
                "      \"text_sha256\": \"{}\"\n",
                hex(digest.as_bytes())
            ));
        }
        out.push_str("    },\n");
    }
    out.push_str("    {\n");
    out.push_str("      \"name\": \"recovered_startxref\",\n");
    out.push_str(
        "      \"why\": \"synthetic document whose startxref offset is garbage: the xref \
is rebuilt by scanning and the rebuild is reported, never silent\",\n",
    );
    {
        let data = recovery_document_bytes();
        let doc = Document::open(&data)
            .unwrap_or_else(|e| panic!("recovery document must open via scan rebuild: {e}"));
        let text = doc.text().expect("recovery document extracts");
        let digest = sha256(text.as_bytes()).expect("sha256 of recovery text");
        out.push_str(&format!("      \"pages\": {},\n", doc.pages()));
        out.push_str(&format!("      \"rebuilt\": {},\n", doc.xref_was_rebuilt()));
        out.push_str(&format!("      \"text_bytes\": {},\n", text.len()));
        out.push_str(&format!(
            "      \"text_sha256\": \"{}\",\n",
            hex(digest.as_bytes())
        ));
        out.push_str(&format!("      \"input\": \"{}\"\n", hex(&data)));
    }
    out.push_str("    }\n");
    out.push_str("  ],\n");

    out.push_str("  \"errors\": [\n");
    for (ei, (name, why, api, input, obj)) in vectors.iter().enumerate() {
        let observed = run_error_vector(api, obj, input);
        let expected = expected_error(name);
        assert_eq!(
            &observed, expected,
            "error vector {name} refusal message drifted from its pinned string"
        );
        out.push_str("    {\n");
        out.push_str(&format!("      \"name\": \"{name}\",\n"));
        out.push_str(&format!("      \"why\": \"{}\",\n", json_escape(why)));
        out.push_str(&format!("      \"api\": \"{api}\",\n"));
        out.push_str(&format!("      \"input\": \"{}\",\n", hex(input)));
        out.push_str(&format!("      \"error\": \"{}\"\n", json_escape(expected)));
        out.push_str("    }");
        if ei + 1 < vectors.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("  ]\n");
    out.push_str("}\n");
    out
}

/// Writes `reference.json`. Returns the number of pinned vectors (fixtures
/// + recovery + refusals).
fn generate_at(path: &Path) -> std::io::Result<usize> {
    let json = reference_json();
    fs::write(path, json)?;
    Ok(FIXTURES.len() + 1 + error_vectors().len())
}

/// Recomputes the reference and compares it byte-for-byte with the
/// committed copy. `Ok(vectors)` means the committed file is current.
fn verify_at(path: &Path) -> Result<usize, String> {
    let committed =
        fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let recomputed = reference_json();
    if committed != recomputed {
        let first = committed
            .bytes()
            .zip(recomputed.bytes())
            .position(|(a, b)| a != b)
            .unwrap_or(committed.len().min(recomputed.len()));
        return Err(format!(
            "recomputed reference differs from {} (first difference at byte {})",
            path.display(),
            first
        ));
    }
    Ok(FIXTURES.len() + 1 + error_vectors().len())
}

/// The CLI body: the mode argument against the reference path. Returns the
/// process exit code so tests can exercise every branch in-process; `main`
/// is the only thing that actually exits.
fn run(path: &Path, mode: Option<&str>) -> i32 {
    match mode {
        Some("gen") => match generate_at(path) {
            Ok(vectors) => {
                println!(
                    "wrote {} ({} fixtures, {} vectors)",
                    path.display(),
                    FIXTURES.len(),
                    vectors
                );
                0
            }
            Err(e) => {
                eprintln!("gen-reference: {e}");
                1
            }
        },
        Some("verify") => match verify_at(path) {
            Ok(n) => {
                println!("reference.json is current ({n} vectors verified)");
                0
            }
            Err(e) => {
                eprintln!("reference.json is stale: {e}");
                1
            }
        },
        other => {
            eprintln!("usage: gen-reference <gen|verify> (got {other:?})");
            2
        }
    }
}

fn main() {
    std::process::exit(run(&reference_path(), std::env::args().nth(1).as_deref()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use pith_pdf::Ref;

    /// A scratch path unique to this test process, cleaned up by the caller.
    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("gen-reference-{}-{name}", std::process::id()))
    }

    #[test]
    fn json_escape_escapes_specials() {
        assert_eq!(json_escape("plain"), "plain");
        assert_eq!(json_escape("a\"b\\c\nd\te\rf"), "a\\\"b\\\\c\\nd\\te\\rf");
        assert_eq!(json_escape("\u{1}"), "\\u0001");
    }

    #[test]
    fn corpus_declares_the_committed_fixtures() {
        // Every corpus entry must have a real fixture pair, and the
        // directory must hold nothing the corpus does not account for
        // (prose such as PROVENANCE.md is not a fixture).
        let mut committed: Vec<_> = fs::read_dir(fixtures_dir())
            .expect("fixtures directory exists")
            .map(|e| {
                e.expect("dirent")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .filter(|n| n.ends_with(".pdf"))
            .collect();
        committed.sort();
        let mut declared: Vec<_> = FIXTURES.iter().map(|f| format!("{}.pdf", f.stem)).collect();
        declared.sort();
        assert_eq!(committed, declared, "fixture files and corpus disagree");
        for fixture in FIXTURES {
            let data = read_fixture(fixture.stem);
            assert!(
                !data.is_empty(),
                "fixture {} is empty on disk",
                fixture.stem
            );
            assert!(
                fixture.encrypted
                    || fixtures_dir()
                        .join(format!("{}.txt", fixture.stem))
                        .exists(),
                "fixture {} has no .txt truth file",
                fixture.stem
            );
        }
    }

    #[test]
    fn every_fixture_matches_its_truth_file() {
        for fixture in FIXTURES {
            if fixture.encrypted {
                continue; // the encrypted fixture pins a refusal, not text
            }
            let data = read_fixture(fixture.stem);
            let doc = open_fixture(fixture.stem, &data);
            extract_fixture(fixture.stem, &doc);
        }
    }

    #[test]
    fn encrypted_fixture_refuses_with_page_context() {
        let data = read_fixture("encrypted");
        let doc = open_fixture("encrypted", &data);
        assert!(doc.is_encrypted());
        let err = doc.text().expect_err("encrypted document must refuse");
        let msg = format!("{err}");
        assert!(
            msg.contains("page 0") && msg.contains("encrypted"),
            "refusal must carry page context: {msg}"
        );
    }

    #[test]
    fn reference_pins_every_fixture_and_error() {
        let json = reference_json();
        let errors = error_vectors();
        assert_eq!(
            json.matches("\"name\":").count(),
            FIXTURES.len() + 1 + errors.len()
        );
        assert_eq!(
            json.matches("\"text_sha256\":").count(),
            FIXTURES.len() + 1 - 1 // every fixture but the encrypted one, plus recovery
        );
        assert_eq!(json.matches("\"error\":").count(), errors.len() + 1);
        // The recovery vector is pinned with the rebuild flag asserted.
        assert!(json.contains("\"name\": \"recovered_startxref\""));
        let data = recovery_document_bytes();
        let doc = Document::open(&data).expect("recovery document opens");
        assert!(doc.xref_was_rebuilt());
        // Every corpus `why` is carried through verbatim.
        for fixture in FIXTURES {
            assert!(json.contains(&format!("\"why\": \"{}\"", json_escape(fixture.why))));
        }
    }

    #[test]
    fn generate_then_verify_round_trips() {
        let path = scratch("roundtrip.json");
        let vectors = generate_at(&path).expect("write reference");
        assert_eq!(vectors, FIXTURES.len() + 1 + error_vectors().len());
        assert_eq!(verify_at(&path), Ok(vectors));
        let written = fs::read_to_string(&path).expect("read back");
        assert_eq!(written, reference_json());
        assert!(written.ends_with("}\n"));
        fs::remove_file(&path).ok();
    }

    #[test]
    fn verify_rejects_a_tampered_reference() {
        let path = scratch("tamper.json");
        let mut json = reference_json();
        // Corrupt one JSON key: any byte drift must fail the verify.
        let pos = json.find("\"text_sha256\"").expect("digest field present");
        json.replace_range(pos..pos + 10, "\"text_sha255\"");
        fs::write(&path, json).expect("write tampered reference");
        assert!(verify_at(&path).is_err());
        fs::remove_file(&path).ok();
    }

    #[test]
    fn verify_reports_a_missing_file() {
        let path = scratch("missing-does-not-exist.json");
        fs::remove_file(&path).ok();
        let err = verify_at(&path).expect_err("missing file must fail");
        assert!(err.contains("cannot read"));
    }

    #[test]
    fn run_dispatches_gen_verify_and_usage() {
        let path = scratch("run.json");
        fs::remove_file(&path).ok();
        assert_eq!(run(&path, Some("gen")), 0);
        assert!(path.exists());
        assert_eq!(run(&path, Some("verify")), 0);
        assert_eq!(run(&path, Some("polish")), 2);
        assert_eq!(run(&path, None), 2);
        fs::remove_file(&path).ok();
    }

    #[test]
    fn run_verify_fails_on_a_stale_reference() {
        let path = scratch("run-stale.json");
        fs::write(&path, "{\n  \"schema\": 0\n}\n").expect("write stale reference");
        assert_eq!(run(&path, Some("verify")), 1);
        fs::remove_file(&path).ok();
    }

    #[test]
    fn reference_path_lands_beside_the_manifest() {
        assert!(reference_path().is_absolute());
        assert_eq!(
            reference_path().file_name(),
            Some("reference.json".as_ref())
        );
    }

    #[test]
    fn fixtures_dir_lands_under_tests() {
        assert!(fixtures_dir().is_absolute());
        assert_eq!(fixtures_dir().file_name(), Some("fixtures".as_ref()));
    }

    #[test]
    #[should_panic(expected = "failed to open")]
    fn an_unopenable_fixture_is_a_broken_build() {
        let _ = open_fixture("not-a-fixture", b"junk that is not a pdf at all");
    }

    #[test]
    fn the_empty_page_fixture_extracts_to_join_only_text() {
        let data = read_fixture("empty_page");
        let doc = open_fixture("empty_page", &data);
        assert_eq!(doc.pages(), 2);
        // One text page, one content-less page: the tail of the output is
        // the documented \x0c page join itself.
        assert_eq!(
            doc.text().expect("empty pages extract"),
            "only page one\x0c"
        );
    }

    #[test]
    fn every_error_vector_refuses_with_its_pinned_message() {
        for (name, _why, api, input, obj) in error_vectors() {
            let observed = run_error_vector(api, &obj, &input);
            assert_eq!(observed, expected_error(name), "vector {name} drifted");
        }
    }

    #[test]
    fn zlib_payload_decodes_through_the_suite() {
        // The predictor refusal vectors depend on this exact byte string
        // being a valid zlib stream; a typo there would flip their
        // refusal into an inflate error.
        let decoded = pith_inflate::inflate_zlib(ZLIB_PAYLOAD, &Limits::default())
            .expect("ZLIB_PAYLOAD must be a valid zlib stream");
        assert_eq!(decoded, b"predictor");
    }

    #[test]
    fn direct_object_lookup_round_trips() {
        // The public Obj accessors the error vectors are built with stay
        // honest: a dict lookup returns the inserted value and a Ref
        // round-trips its fields.
        let d = dict(&[(b"Filter", name(b"LZWDecode"))]);
        assert_eq!(
            d.get(b"Filter").and_then(Obj::as_bytes),
            Some(&b"LZWDecode"[..])
        );
        assert!(d.get(b"Missing").is_none());
        let r = Ref {
            num: 7,
            generation: 1,
        };
        let o = Obj::Ref(r);
        assert_eq!(o.as_ref(), Some(r));
    }
}
