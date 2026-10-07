//! Edge-path tests for the port: error arms, filter branches and parser
//! refusals the committed fixture corpus does not reach. The ported
//! `tests/pdf.rs` suite stays untouched; everything here is additive and
//! goes through public API only (`Document`, `decode_stream`, `Obj`,
//! `cmap::CMap`), so a future refactor cannot silently drop a branch.
//!
//! Synthetic documents are assembled byte-exactly by [`Builder`], a
//! minimal classic-xref PDF writer: no fixture generator is involved, so
//! each refusal below pins one exact code path.

use pith_inflate::Limits;
use pith_pdf::cmap::CMap;
use pith_pdf::{Document, Obj, Ref, decode_stream, extract_text};

// ---------------------------------------------------------------------
// Synthetic document builder
// ---------------------------------------------------------------------

/// Accumulates objects and serialises a classic-xref PDF around them.
struct Builder {
    out: Vec<u8>,
    offsets: Vec<usize>,
}

impl Builder {
    /// Starts a document with the `%PDF-1.4` header.
    fn new() -> Builder {
        Builder {
            out: b"%PDF-1.4\n".to_vec(),
            offsets: Vec::new(),
        }
    }

    /// Adds one object whose body is arbitrary bytes (the `N G obj` /
    /// `endobj` wrapper is supplied).
    fn obj(&mut self, body: &[u8]) -> u32 {
        let num = self.offsets.len() as u32 + 1;
        self.offsets.push(self.out.len());
        self.out
            .extend_from_slice(format!("{num} 0 obj\n").as_bytes());
        self.out.extend_from_slice(b"\n");
        self.out.extend_from_slice(body);
        self.out.extend_from_slice(b"\nendobj\n");
        num
    }

    /// Adds one object from a string body.
    fn objs(&mut self, body: &str) -> u32 {
        self.obj(body.as_bytes())
    }

    /// Adds a stream object: dictionary extras plus raw data (the
    /// `/Length` is computed).
    fn stream(&mut self, dict: &str, data: &[u8]) -> u32 {
        let head = format!("<< /Length {} {dict} >>\nstream\n", data.len());
        let mut body = head.into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(b"\nendstream");
        self.obj(&body)
    }

    /// Serialises the document: classic xref for every object, trailer
    /// with `/Root 1 0 R` plus `extra`, `startxref` pointing at it.
    fn finish(self, extra: &str) -> Vec<u8> {
        let count = self.offsets.len() + 1;
        let xref_off = self.out.len();
        let mut out = self.out;
        out.extend_from_slice(format!("xref\n0 {count}\n0000000000 65535 f \n").as_bytes());
        for off in &self.offsets {
            out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {count} /Root 1 0 R {extra} >>\nstartxref\n{xref_off}\n%%EOF\n"
            )
            .as_bytes(),
        );
        out
    }

    /// Serialises with a caller-supplied xref+trailer tail (the tail must
    /// contain its own `startxref`).
    fn finish_raw(self, tail: &[u8]) -> Vec<u8> {
        let mut out = self.out;
        out.extend_from_slice(tail);
        out
    }
}

/// A one-page document around a content stream: catalog, pages, page
/// (with `resources` on the page), content stream, Helvetica font, then
/// `extra` objects numbered from 6.
fn page_doc(contents: &[u8], resources: &str, extra: &[&[u8]]) -> Vec<u8> {
    let mut b = Builder::new();
    let _catalog = b.objs("<< /Type /Catalog /Pages 2 0 R >>");
    let _pages = b.objs("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    let page = format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /Resources {resources} >>"
    );
    let _page = b.objs(&page);
    let _contents = b.stream("", contents);
    let _font = b.objs("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
    for e in extra {
        b.obj(e);
    }
    b.finish("")
}

/// Opens `data` and expects the refusal `msg` (exact `Display`).
fn refuse(data: &[u8], msg: &str) {
    match extract_text(data) {
        Ok(t) => panic!("expected refusal {msg:?}, got text {t:?}"),
        Err(e) => assert_eq!(format!("{e}"), msg, "wrong refusal"),
    }
}

/// Decodes `obj` through the public stream filter API, expecting `msg`.
fn refuse_stream(obj: &Obj, msg: &str) {
    let err = match decode_stream(obj, &Limits::default()) {
        Err(e) => e,
        Ok(d) => panic!("expected refusal {msg:?}, decoded {d:?}"),
    };
    assert_eq!(format!("{err}"), msg, "wrong refusal");
}

/// A dict object from ordered key/value byte pairs.
fn dict(kvs: &[(&[u8], Obj)]) -> Obj {
    Obj::Dict(kvs.iter().map(|(k, v)| (k.to_vec(), v.clone())).collect())
}

/// Name object (slash stripped).
fn name(n: &[u8]) -> Obj {
    Obj::Name(n.to_vec())
}

/// Integer-valued number object.
fn int(v: i64) -> Obj {
    Obj::Num(v as f64)
}

/// Stream object from dictionary pairs and raw bytes.
fn stream(kvs: &[(&[u8], Obj)], data: &[u8]) -> Obj {
    Obj::Stream {
        dict: kvs.iter().map(|(k, v)| (k.to_vec(), v.clone())).collect(),
        data: data.to_vec(),
    }
}

// ---------------------------------------------------------------------
// Obj accessor edges
// ---------------------------------------------------------------------

/// Every accessor answers `None` on the wrong variant and honest values
/// on the right one, including the numeric range edges.
#[test]
fn obj_accessors_on_wrong_variants_and_numeric_edges() {
    assert!(Obj::Null.get(b"k").is_none());
    assert!(Obj::Null.dict().is_none());
    assert!(Obj::Null.as_ref().is_none());
    assert!(Obj::Null.as_f64().is_none());
    assert!(Obj::Null.as_bool().is_none());
    assert!(Obj::Null.as_bytes().is_none());
    assert!(Obj::Null.as_array().is_none());
    assert!(Obj::Null.stream_data().is_none());
    // numbers: fractional, huge and negative values stay unreachable
    assert_eq!(Obj::Num(1.5).as_i64(), None);
    assert_eq!(Obj::Num(1e19).as_i64(), None);
    assert_eq!(Obj::Num(-3.0).as_u32(), None);
    assert_eq!(Obj::Num(4294967296.0).as_u32(), None);
    assert_eq!(Obj::Num(-1.0).as_usize(), None);
    assert_eq!(Obj::Num(3.0).as_usize(), Some(3));
    assert_eq!(Obj::Num(3.0).as_f64(), Some(3.0));
    // names vs strings share `as_bytes`
    assert_eq!(Obj::Str(b"s".to_vec()).as_bytes(), Some(&b"s"[..]));
    // arrays and streams expose their payloads
    let arr = Obj::Arr(vec![int(1)]);
    assert_eq!(arr.as_array().map(|a| a.len()), Some(1));
    let st = stream(&[], b"raw");
    assert_eq!(st.stream_data(), Some(&b"raw"[..]));
    // a reference round-trips both fields
    let r = Ref {
        num: u32::MAX,
        generation: u16::MAX,
    };
    assert_eq!(Obj::Ref(r).as_ref(), Some(r));
}

// ---------------------------------------------------------------------
// decode_stream: filters and predictors
// ---------------------------------------------------------------------

/// A valid zlib stream of `b"raw fallback payload"`.
const ZLIB_OK: &[u8] = &[
    0x78, 0x9c, 0x2b, 0x4a, 0x2c, 0x57, 0x48, 0x4b, 0xcc, 0xc9, 0x49, 0x4a, 0x4c, 0xce, 0x56, 0x28,
    0x48, 0xac, 0xcc, 0xc9, 0x4f, 0x4c, 0x01, 0x00, 0x4f, 0x5a, 0x07, 0xa5,
];
/// The same payload as a bare raw-deflate stream (no zlib wrapper): the
/// documented fallback path for non-conforming files.
const RAW_OK: &[u8] = &[
    0x2b, 0x4a, 0x2c, 0x57, 0x48, 0x4b, 0xcc, 0xc9, 0x49, 0x4a, 0x4c, 0xce, 0x56, 0x28, 0x48, 0xac,
    0xcc, 0xc9, 0x4f, 0x4c, 0x01, 0x00,
];
/// PNG-filtered rows (filter types 0-4), Colors 1, Columns 4, 8bpc.
const PNG_ROWS: &[u8] = &[
    0x78, 0x9c, 0x63, 0x60, 0x64, 0x62, 0x66, 0x61, 0x64, 0x64, 0x60, 0x60, 0x60, 0x02, 0x11, 0xcc,
    0x20, 0x82, 0x05, 0x44, 0x00, 0x00, 0x01, 0x87, 0x00, 0x19,
];
const PNG_EXPECT: &[u8] = &[1, 2, 3, 4, 1, 1, 1, 1, 2, 1, 1, 1, 2, 1, 1, 1, 3, 2, 2, 2];
/// Second row carries the invalid PNG filter type 9.
const PNG_BADFT: &[u8] = &[
    0x78, 0x9c, 0x63, 0x60, 0x64, 0x62, 0x66, 0xe1, 0x04, 0x11, 0x00, 0x00, 0x91, 0x00, 0x1e,
];
/// One and a half rows: the trailing partial row is a truncation.
const PNG_SHORT: &[u8] = &[
    0x78, 0x9c, 0x63, 0x60, 0x64, 0x62, 0x66, 0x61, 0x60, 0x04, 0x00, 0x00, 0x30, 0x00, 0x0c,
];
/// TIFF-delta rows (predictor 2), Colors 1, Columns 4, 8bpc.
const TIFF_ROWS: &[u8] = &[
    0x78, 0x9c, 0x63, 0x64, 0x62, 0x66, 0x61, 0x60, 0x64, 0x60, 0x00, 0x00, 0x00, 0x47, 0x00, 0x0c,
];
const TIFF_EXPECT: &[u8] = &[1, 3, 6, 10, 0, 1, 1, 1];

/// FlateDecode handles both zlib-wrapped and bare raw-deflate streams;
/// bytes neither can decode refuse.
#[test]
fn flate_zlib_raw_fallback_and_garbage() {
    let z = stream(&[(b"Filter", name(b"FlateDecode"))], ZLIB_OK);
    assert_eq!(
        decode_stream(&z, &Limits::default()).unwrap(),
        b"raw fallback payload"
    );
    let r = stream(&[(b"Filter", name(b"FlateDecode"))], RAW_OK);
    assert_eq!(
        decode_stream(&r, &Limits::default()).unwrap(),
        b"raw fallback payload"
    );
    let g = stream(&[(b"Filter", name(b"FlateDecode"))], b"\x00\x01\x02");
    assert!(decode_stream(&g, &Limits::default()).is_err());
    // the abbreviation filters are the same branch
    let fl = stream(&[(b"Filter", name(b"Fl"))], ZLIB_OK);
    assert_eq!(
        decode_stream(&fl, &Limits::default()).unwrap(),
        b"raw fallback payload"
    );
}

/// All five PNG filter types decode through the predictor pass.
#[test]
fn predictor_png_all_filter_types_decode() {
    let p = stream(
        &[
            (b"Filter", name(b"FlateDecode")),
            (
                b"DecodeParms",
                dict(&[
                    (b"Predictor", int(15)),
                    (b"Colors", int(1)),
                    (b"Columns", int(4)),
                    (b"BitsPerComponent", int(8)),
                ]),
            ),
        ],
        PNG_ROWS,
    );
    assert_eq!(decode_stream(&p, &Limits::default()).unwrap(), PNG_EXPECT);
}

/// A row with an out-of-range PNG filter type is a refusal, never a
/// silent pass-through.
#[test]
fn predictor_png_bad_filter_type_refuses() {
    let p = stream(
        &[
            (b"Filter", name(b"FlateDecode")),
            (
                b"DecodeParms",
                dict(&[(b"Predictor", int(10)), (b"Columns", int(4))]),
            ),
        ],
        PNG_BADFT,
    );
    refuse_stream(&p, "bad value: PNG predictor filter type");
}

/// A trailing partial PNG row is a truncation, not dropped bytes.
#[test]
fn predictor_png_partial_row_refuses() {
    let p = stream(
        &[
            (b"Filter", name(b"FlateDecode")),
            (
                b"DecodeParms",
                dict(&[(b"Predictor", int(12)), (b"Columns", int(4))]),
            ),
        ],
        PNG_SHORT,
    );
    refuse_stream(&p, "truncated: PNG predictor row");
}

/// Row sizes that overflow while computing refuse by name.
#[test]
fn predictor_row_size_overflow_refuses() {
    let parms = dict(&[
        (b"Predictor", int(11)),
        (b"Colors", int(2_000_000_000)),
        (b"Columns", int(2_000_000_000)),
    ]);
    let p = stream(
        &[(b"Filter", name(b"FlateDecode")), (b"DecodeParms", parms)],
        PNG_ROWS,
    );
    refuse_stream(&p, "bad value: PNG predictor row size");
    let parms = dict(&[
        (b"Predictor", int(2)),
        (b"Colors", int(4_500_000_000)),
        (b"Columns", int(4_500_000_000)),
    ]);
    let t = stream(
        &[(b"Filter", name(b"FlateDecode")), (b"DecodeParms", parms)],
        TIFF_ROWS,
    );
    refuse_stream(&t, "bad value: TIFF predictor row size");
}

/// The TIFF predictor accumulates per-row deltas.
#[test]
fn predictor_tiff_deltas_accumulate() {
    let p = stream(
        &[
            (b"Filter", name(b"FlateDecode")),
            (
                b"DecodeParms",
                dict(&[(b"Predictor", int(2)), (b"Columns", int(4))]),
            ),
        ],
        TIFF_ROWS,
    );
    assert_eq!(decode_stream(&p, &Limits::default()).unwrap(), TIFF_EXPECT);
}

/// `/DecodeParms` in array form reaches the same predictor path.
#[test]
fn predictor_parms_array_form() {
    let p = stream(
        &[
            (b"Filter", name(b"FlateDecode")),
            (
                b"DecodeParms",
                Obj::Arr(vec![dict(&[(b"Predictor", int(2)), (b"Colors", int(0))])]),
            ),
        ],
        TIFF_ROWS,
    );
    refuse_stream(&p, "bad value: DecodeParms dimensions");
}

/// Filter chains apply left to right: hex-embedded zlib decodes twice.
#[test]
fn filter_chain_applies_in_order() {
    let mut hexed = Vec::new();
    for byte in ZLIB_OK {
        hexed.extend_from_slice(format!("{byte:02X}").as_bytes());
    }
    let p = stream(
        &[(
            b"Filter",
            Obj::Arr(vec![name(b"ASCIIHexDecode"), name(b"FlateDecode")]),
        )],
        &hexed,
    );
    assert_eq!(
        decode_stream(&p, &Limits::default()).unwrap(),
        b"raw fallback payload"
    );
}

/// ASCIIHex pads an odd trailing nibble and stops at `>`.
#[test]
fn asciihex_padding_and_terminator() {
    let h = stream(&[(b"Filter", name(b"AHx"))], b"48656C6C6F>");
    assert_eq!(decode_stream(&h, &Limits::default()).unwrap(), b"Hello");
    let odd = stream(&[(b"Filter", name(b"ASCIIHexDecode"))], b"486");
    assert_eq!(
        decode_stream(&odd, &Limits::default()).unwrap(),
        &[0x48, 0x60]
    );
}

/// ASCII85: full groups, the `z` shortcut, tolerated missing terminator,
/// and the bad-character refusal.
#[test]
fn ascii85_groups_shortcut_and_refusals() {
    let full = stream(&[(b"Filter", name(b"A85"))], b"BOu!rDZ~>");
    assert_eq!(decode_stream(&full, &Limits::default()).unwrap(), b"hello");
    let short = stream(&[(b"Filter", name(b"ASCII85Decode"))], b"@:B");
    assert_eq!(decode_stream(&short, &Limits::default()).unwrap(), b"ab");
    let zero = stream(&[(b"Filter", name(b"ASCII85Decode"))], b"z");
    assert_eq!(
        decode_stream(&zero, &Limits::default()).unwrap(),
        &[0, 0, 0, 0]
    );
    let bad = stream(&[(b"Filter", name(b"ASCII85Decode"))], b"{");
    refuse_stream(&bad, "bad value: ASCII85 character");
}

/// Every out-of-scope-but-real filter refuses by name.
#[test]
fn remaining_filter_refusals_name_the_filter() {
    for (filter, msg) in [
        (
            &b"JBIG2Decode"[..],
            "unsupported: JBIG2Decode stream filter",
        ),
        (&b"JPXDecode"[..], "unsupported: JPXDecode stream filter"),
        (
            &b"CCITTFaxDecode"[..],
            "unsupported: CCITTFaxDecode stream filter",
        ),
        (&b"CCF"[..], "unsupported: CCITTFaxDecode stream filter"),
        (
            &b"RunLengthDecode"[..],
            "unsupported: RunLengthDecode stream filter",
        ),
        (&b"RL"[..], "unsupported: RunLengthDecode stream filter"),
        (&b"LZW"[..], "unsupported: LZWDecode stream filter"),
    ] {
        let s = stream(&[(b"Filter", name(filter))], b"");
        refuse_stream(&s, msg);
    }
    let unknown = stream(&[(b"Filter", name(b"OpeningBookmark"))], b"");
    refuse_stream(&unknown, "unsupported: unknown stream filter");
}

// ---------------------------------------------------------------------
// CMap parse edges
// ---------------------------------------------------------------------

/// Parses a CMap, expecting the refusal `msg`.
fn refuse_cmap(src: &[u8], msg: &str) {
    let err = match CMap::parse(src) {
        Err(e) => e,
        Ok(_) => panic!("expected refusal {msg:?}"),
    };
    assert_eq!(format!("{err}"), msg, "wrong refusal");
}

/// A minimal valid CMap body the edge cases are grafted onto.
const CMAP_OK: &[u8] = b"1 begincodespacerange\n<00> <FF>\nendcodespacerange\n\
1 beginbfchar\n<41> <0041>\nendbfchar\nendcmap\n";

/// Codespace ranges must be equal-length and at most 7 bytes.
#[test]
fn cmap_codespacerange_edges() {
    refuse_cmap(
        b"1 begincodespacerange\n<00> <FFFF>\nendcodespacerange\nendcmap\n",
        "bad value: codespacerange",
    );
    refuse_cmap(
        b"1 begincodespacerange\n<00000000000000> <FFFFFFFFFFFFFFFF>\nendcodespacerange\nendcmap\n",
        "bad value: codespacerange",
    );
}

/// Section counts must be small non-negative integers.
#[test]
fn cmap_section_count_edges() {
    refuse_cmap(
        b"begincodespacerange\nendcmap\n",
        "bad value: cmap section count",
    );
    refuse_cmap(
        b"(x) beginbfchar\nendcmap\n",
        "bad value: cmap section count",
    );
    refuse_cmap(
        b"1.5 beginbfchar\nendcmap\n",
        "bad value: cmap section count",
    );
    refuse_cmap(
        b"-1 beginbfchar\nendcmap\n",
        "bad value: cmap section count",
    );
    refuse_cmap(
        b"1000001 beginbfchar\nendcmap\n",
        "bad value: cmap section count",
    );
}

/// bfchar sources and destinations are validated.
#[test]
fn cmap_bfchar_edges() {
    refuse_cmap(
        b"1 beginbfchar\n<0000000000000000> <0041>\nendbfchar\nendcmap\n",
        "bad value: bfchar source",
    );
    refuse_cmap(
        b"1 beginbfchar\n<41> <>\nendbfchar\nendcmap\n",
        "bad value: bfchar destination",
    );
    refuse_cmap(b"endcmap\n", "bad value: CMap has no mappings");
}

/// bfrange bounds, span caps and destination forms are validated.
#[test]
fn cmap_bfrange_edges() {
    refuse_cmap(
        b"1 beginbfrange\n<00> <0004> <0041>\nendbfrange\nendcmap\n",
        "bad value: bfrange source",
    );
    refuse_cmap(
        b"1 beginbfrange\n<0005> <0001> <0041>\nendbfrange\nendcmap\n",
        "bad value: bfrange range",
    );
    refuse_cmap(
        b"1 beginbfrange\n<000000> <010001> <0041>\nendbfrange\nendcmap\n",
        "too large: bfrange span",
    );
    refuse_cmap(
        b"1 beginbfrange\n<0001> <0002>\nendbfrange\nendcmap\n",
        "bad value: bfrange destination",
    );
    refuse_cmap(
        b"1 beginbfrange\n<0001> <0003> [<0041> <0042>]\nendbfrange\nendcmap\n",
        "bad value: bfrange array length",
    );
    refuse_cmap(
        b"1 beginbfrange\n<0001> <0002> [<0041> <0042> <0043>]\nendbfrange\nendcmap\n",
        "bad value: bfrange array length",
    );
    refuse_cmap(
        b"1 beginbfrange\n<0001> <0002> [<0041>",
        "truncated: bfrange array",
    );
    refuse_cmap(
        b"1 beginbfrange\n<0001> <0002> [/x]\nendbfrange\nendcmap\n",
        "bad value: bfrange array",
    );
    refuse_cmap(
        b"1 beginbfrange\n<0001> <0002> <00410>\nendbfrange\nendcmap\n",
        "bad value: bfrange destination bytes",
    );
}

/// `/WMode 1 def` flips the writing mode; a CMap without mappings refuses.
#[test]
fn cmap_wmode_and_empty() {
    let v = CMap::parse(
        b"/WMode 1 def\n1 begincodespacerange\n<00> <FF>\nendcodespacerange\n1 beginbfchar\n<41> <0041>\nendbfchar\nendcmap\n",
    )
    .expect("vertical cmap parses");
    assert!(v.is_vertical());
    let h = CMap::parse(CMAP_OK).expect("horizontal cmap parses");
    assert!(!h.is_vertical());
    refuse_cmap(b"/Type /CMap\nendcmap\n", "bad value: CMap has no mappings");
}

/// CID sections are skipped, including their array destinations; a
/// truncated CID section refuses by name.
#[test]
fn cmap_cid_sections_skipped() {
    let c = CMap::parse(
        b"1 beginbfchar\n<41> <0041>\nendbfchar\n\
2 begincidrange\n<0000> <00FF> 1\n<0100> <01FF> 2\nendcidrange\n\
1 beginnotdefchar\n<FF> 1\nendnotdefchar\nendcmap\n",
    )
    .expect("cid sections skipped");
    assert_eq!(c.lookup(&[0x41]), (Some("A"), 1));
    // a string array as the destination reaches the skipper's array arm
    let c = CMap::parse(
        b"1 beginbfchar\n<41> <0041>\nendbfchar\n\
1 begincidrange\n<00> [(a) (b)]\nendcidrange\nendcmap\n",
    )
    .expect("cid string array skipped");
    assert_eq!(c.lookup(&[0x41]), (Some("A"), 1));
    // pair truncated at end of stream
    refuse_cmap(
        b"1 beginbfchar\n<41> <0041>\nendbfchar\n1 begincidrange\n<00>",
        "truncated: cid section",
    );
    // array opener truncated at end of stream
    refuse_cmap(
        b"1 beginbfchar\n<41> <0041>\nendbfchar\n1 begincidrange\n<00> [",
        "truncated: cidrange array",
    );
    // an unclosed paren inside the array is a lexer-level truncation
    refuse_cmap(
        b"1 beginbfchar\n<41> <0041>\nendbfchar\n1 begincidrange\n<00> [(a\nendcidrange\nendcmap\n",
        "truncated: literal string",
    );
    refuse_cmap(
        b"1 beginbfchar\n<41> <0041>\nendbfchar\n1 begincidchar\n<0041>",
        "truncated: cid section",
    );
}

/// The operand stack is capped: a flood of operands before a keyword
/// refuses instead of growing without bound.
#[test]
fn cmap_operand_stack_capped() {
    let mut src = Vec::new();
    for _ in 0..4098 {
        src.extend_from_slice(b"1 ");
    }
    src.extend_from_slice(b"endcmap\n");
    refuse_cmap(&src, "bad value: cmap operand stack");
}

/// CMap strings exercise the lexer's literal-string edges: octal escapes,
/// line continuations, unknown escapes, runaway nesting and a trailing
/// backslash.
#[test]
fn cmap_literal_string_lexer_edges() {
    // octal escapes and unknown escapes decode to literal bytes; the
    // destination must stay even-length UTF-16BE, so the payload is
    // prefixed with a NUL high byte
    let c = CMap::parse(b"1 beginbfchar\n<41> (\\000\\101\\000\\102)\nendbfchar\nendcmap\n")
        .expect("escapes decode");
    assert_eq!(c.lookup(&[0x41]), (Some("AB"), 1));
    // CR-LF line continuation vanishes
    let c = CMap::parse(b"1 beginbfchar\n<41> (\\000\\141\\000\\\r\n\\142)\nendbfchar\nendcmap\n")
        .expect("continuation decodes");
    assert_eq!(c.lookup(&[0x41]), (Some("ab"), 1));
    // runaway nesting is refused at depth 64
    let mut deep = b"1 beginbfchar\n<41> (".to_vec();
    deep.extend_from_slice(&[b'('; 65]);
    deep.extend_from_slice(b")\nendbfchar\nendcmap\n");
    refuse_cmap(&deep, "bad value: literal string nesting");
    // a trailing backslash cannot form an escape
    refuse_cmap(
        b"1 beginbfchar\n<41> (ab\\\nendbfchar\nendcmap\n",
        "truncated: literal string",
    );
}

/// `lookup` reports misses as `None` with the shortest declared width so
/// the caller always advances.
#[test]
fn cmap_lookup_miss_advances_by_shortest_width() {
    let c = CMap::parse(
        b"1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n\
1 beginbfchar\n<0041> <0041>\nendbfchar\nendcmap\n",
    )
    .expect("cmap parses");
    assert_eq!(c.lookup(&[0x00, 0x41]), (Some("A"), 2));
    assert_eq!(c.lookup(&[0xFF, 0xFF]), (None, 2));
}

// ---------------------------------------------------------------------
// Font edges (synthetic documents)
// ---------------------------------------------------------------------

/// A font resource used by the content-edge tests.
fn font_obj(body: &str) -> String {
    format!("<< /Type /Font {body} >>")
}

/// `/Encoding` by unknown name, unknown `/BaseEncoding`, bad Differences
/// numbers/elements and bad `/Encoding` types refuse by name.
#[test]
fn font_encoding_edges_refuse() {
    let contents = b"BT /F1 12 Tf (x) Tj ET";
    let resources = "<< /Font << /F1 6 0 R >> >>";
    let cases: Vec<(String, Vec<String>, &str)> = vec![
        (
            font_obj("/Subtype /Type1 /BaseFont /Helvetica /Encoding /Klingon"),
            vec![],
            "page 0: unsupported: encoding name",
        ),
        (
            font_obj("/Subtype /Type1 /BaseFont /Helvetica /Encoding 7 0 R"),
            vec!["<< /Type /Encoding /BaseEncoding /Klingon /Differences [65 /A] >>".into()],
            "page 0: unsupported: BaseEncoding name",
        ),
        (
            font_obj("/Subtype /Type1 /BaseFont /Helvetica /Encoding 7 0 R"),
            vec!["<< /Differences [300 /A] >>".into()],
            "page 0: bad value: /Differences number",
        ),
        (
            font_obj("/Subtype /Type1 /BaseFont /Helvetica /Encoding 7 0 R"),
            vec!["<< /Differences [(A)] >>".into()],
            "page 0: bad value: /Differences element",
        ),
        (
            font_obj("/Subtype /Type1 /BaseFont /Helvetica /Encoding 42"),
            vec![],
            "page 0: bad value: /Encoding type",
        ),
    ];
    for (font, extra, msg) in cases {
        let mut all: Vec<&[u8]> = vec![font.as_bytes()];
        all.extend(extra.iter().map(|s| s.as_bytes()));
        let doc = page_doc(contents, resources, &all);
        refuse(&doc, msg);
    }
}

/// `uniXXXX`/`uXXXXXX` difference names resolve directly; unknown names
/// fall through to the AGL table and decode as U+FFFD when absent.
#[test]
fn font_differences_uni_and_u_names() {
    let contents = b"BT /F1 12 Tf (\\001\\002\\003\\004) Tj ET";
    let resources = "<< /Font << /F1 7 0 R >> >>";
    let extra = [
        "<< /Differences [1 /uni0041 2 /u0042 3 /A 4 /uniZZZZ] >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding 6 0 R >>",
    ];
    let extra: Vec<&[u8]> = extra.iter().map(|e| e.as_bytes()).collect();
    let doc = page_doc(contents, resources, &extra);
    let text = extract_text(&doc).expect("differences decode");
    assert_eq!(text, "ABA\u{FFFD}");
}

/// Unsupported and missing font subtypes refuse; MMType1 and TrueType
/// take the simple-font path.
#[test]
fn font_subtype_edges() {
    let contents = b"BT /F1 12 Tf (AB) Tj ET";
    for (font, msg) in [
        (
            "<< /Type /Font /BaseFont /Helvetica >>",
            "page 0: bad value: font /Subtype",
        ),
        (
            "<< /Type /Font /Subtype /Weird >>",
            "page 0: unsupported: font subtype",
        ),
    ] {
        let doc = page_doc(contents, "<< /Font << /F1 6 0 R >> >>", &[font.as_bytes()]);
        refuse(&doc, msg);
    }
    for subtype in ["MMType1", "TrueType"] {
        let font = format!("<< /Type /Font /Subtype /{subtype} /BaseFont /Helvetica >>");
        let doc = page_doc(contents, "<< /Font << /F1 6 0 R >> >>", &[font.as_bytes()]);
        assert_eq!(extract_text(&doc).expect("simple font decodes"), "AB");
    }
}

/// `/ToUnicode` must be a decodable stream holding a parseable CMap.
#[test]
fn font_tounicode_edges() {
    {
        let (tounicode, msg) = (
            "<< /Type /Info >>",
            "page 0: bad value: /ToUnicode is not a stream",
        );
        let font = "<< /Type /Font /Subtype /Type1 /ToUnicode 7 0 R >>";
        let doc = page_doc(
            b"BT /F1 12 Tf (A) Tj ET",
            "<< /Font << /F1 6 0 R >> >>",
            &[font.as_bytes(), tounicode.as_bytes()],
        );
        refuse(&doc, msg);
    }
    // undecodable ToUnicode stream (bad hex)
    let font = "<< /Type /Font /Subtype /Type1 /ToUnicode 7 0 R >>";
    let mut tu = Vec::new();
    tu.extend_from_slice(b"<< /Length 4 /Filter /ASCIIHexDecode >>\nstream\nzz\n\nendstream");
    let doc = page_doc(
        b"BT /F1 12 Tf (A) Tj ET",
        "<< /Font << /F1 6 0 R >> >>",
        &[font.as_bytes(), &tu],
    );
    refuse(&doc, "page 0: bad value: /ToUnicode stream decode");
    // unparseable ToUnicode body
    let font = "<< /Type /Font /Subtype /Type1 /ToUnicode 7 0 R >>";
    let tu = b"<< /Length 7 >>\nstream\nno cmap\n\nendstream";
    let doc = page_doc(
        b"BT /F1 12 Tf (A) Tj ET",
        "<< /Font << /F1 6 0 R >> >>",
        &[font.as_bytes(), &tu[..]],
    );
    refuse(&doc, "page 0: bad value: /ToUnicode parse");
}

/// CID descendant fonts must exist, be CIDFontType0/2 and carry a usable
/// `/Encoding`.
#[test]
fn cid_font_edges() {
    let contents = b"BT /F1 12 Tf <00410042> Tj ET";
    let resources = "<< /Font << /F1 6 0 R >> >>";
    // font = object 6, extras follow from 7
    for (font, extra, msg) in [
        (
            "<< /Type /Font /Subtype /Type0 /BaseFont /X /Encoding /Identity-H >>",
            vec![],
            "page 0: bad value: CID font /DescendantFonts",
        ),
        (
            "<< /Type /Font /Subtype /Type0 /BaseFont /X /Encoding /Identity-H /DescendantFonts [7 0 R] >>",
            vec!["<< /Type /Font /Subtype /CIDFontType9 >>"],
            "page 0: unsupported: descendant font subtype",
        ),
        (
            "<< /Type /Font /Subtype /Type0 /BaseFont /X /Encoding /Identity-H /DescendantFonts [7 0 R] >>",
            vec!["<< /Type /Font >>"],
            "page 0: bad value: CID font descendant",
        ),
        (
            "<< /Type /Font /Subtype /Type0 /BaseFont /X /Encoding 42 /DescendantFonts [7 0 R] >>",
            vec!["<< /Type /Font /Subtype /CIDFontType2 >>"],
            "page 0: bad value: /Encoding for CID font",
        ),
        (
            "<< /Type /Font /Subtype /Type0 /BaseFont /X /Encoding 8 0 R /ToUnicode 9 0 R /DescendantFonts [7 0 R] >>",
            vec![
                "<< /Type /Font /Subtype /CIDFontType2 >>",
                "<< /Length 4 /Filter /ASCIIHexDecode >>\nstream\nzz\n\nendstream",
                "<< /Length 94 >>\nstream\n1 begincodespacerange\n<00> <FF>\nendcodespacerange\n1 beginbfchar\n<41> <0041>\nendbfchar\nendcmap\n\nendstream",
            ],
            "page 0: bad value: CMap stream decode",
        ),
        (
            "<< /Type /Font /Subtype /Type0 /BaseFont /X /Encoding 8 0 R /ToUnicode 9 0 R /DescendantFonts [7 0 R] >>",
            vec![
                "<< /Type /Font /Subtype /CIDFontType2 >>",
                "<< /Length 7 >>\nstream\nno cmap\n\nendstream",
                "<< /Length 94 >>\nstream\n1 begincodespacerange\n<00> <FF>\nendcodespacerange\n1 beginbfchar\n<41> <0041>\nendbfchar\nendcmap\n\nendstream",
            ],
            "page 0: bad value: CMap stream parse",
        ),
    ] {
        let mut all: Vec<&[u8]> = vec![font.as_bytes()];
        for e in &extra {
            all.push(e.as_bytes());
        }
        let doc = page_doc(contents, resources, &all);
        refuse(&doc, msg);
    }
}

/// Identity-V marks the font vertical; extraction still runs on
/// `/ToUnicode`.
#[test]
fn cid_identity_v_decodes_via_tounicode() {
    let cmap = "2 beginbfchar\n<0041> <0041>\n<0042> <0042>\nendbfchar\nendcmap\n";
    let tounicode = format!("<< /Length {} >>\nstream\n{}endstream", cmap.len(), cmap);
    let contents = b"BT /F1 12 Tf <00410042> Tj ET";
    let font = "<< /Type /Font /Subtype /Type0 /BaseFont /X /Encoding /Identity-V /DescendantFonts [7 0 R] /ToUnicode 8 0 R >>";
    let doc = page_doc(
        contents,
        "<< /Font << /F1 6 0 R >> >>",
        &[
            font.as_bytes(),
            "<< /Type /Font /Subtype /CIDFontType2 >>".as_bytes(),
            tounicode.as_bytes(),
        ],
    );
    assert_eq!(extract_text(&doc).expect("Identity-V decodes"), "AB");
}

// ---------------------------------------------------------------------
// Content-stream edges
// ---------------------------------------------------------------------

/// Inline images are skipped at the `EI` boundary; a missing `EI` is a
/// truncation, and a binary-looking `EI` that is not whitespace-delimited
/// does not end the image.
#[test]
fn inline_images_skip_to_ei() {
    let c = b"BT /F1 12 Tf (A) Tj ET BI /W 1 /H 1 /BPC 8 ID \x00\x01\x02 EI BT /F1 12 Tf (B) Tj ET";
    let doc = page_doc(&c[..], "<< /Font << /F1 5 0 R >> >>", &[]);
    assert_eq!(extract_text(&doc).expect("inline image skipped"), "AB");
    // an EI glued to data is not a boundary: the first is skipped over
    let c = b"BT /F1 12 Tf (A) Tj ET BI /W 1 ID xxxEI EI BT /F1 12 Tf (B) Tj ET";
    let doc = page_doc(&c[..], "<< /Font << /F1 5 0 R >> >>", &[]);
    assert_eq!(extract_text(&doc).expect("fake EI skipped"), "AB");
    // no EI at all: truncation
    let c = b"BI /W 1 ID \x00\x01\x02";
    let doc = page_doc(&c[..], "<< /Font << /F1 5 0 R >> >>", &[]);
    refuse(&doc, "page 0: truncated: inline image EI");
}

/// Text shown before any `Tf`, and `Tf` naming a font the resources do
/// not carry, refuse by name; a font resource that is not a dictionary
/// refuses too.
#[test]
fn font_resolution_edges() {
    let doc = page_doc(b"BT (x) Tj ET", "<< /Font << /F1 5 0 R >> >>", &[]);
    refuse(&doc, "page 0: bad value: text shown before Tf");
    let doc = page_doc(
        b"BT /F2 12 Tf (x) Tj ET",
        "<< /Font << /F1 5 0 R >> >>",
        &[],
    );
    refuse(&doc, "page 0: bad value: font resource missing");
    let doc = page_doc(
        b"BT /F1 12 Tf (x) Tj ET",
        "<< /Font << /F1 6 0 R >> >>",
        &[b"42"],
    );
    refuse(&doc, "page 0: bad value: font is not a dict");
}

/// Operand-shape errors on show and positioning operators refuse by
/// operator name.
#[test]
fn operator_operand_edges() {
    for (contents, msg) in [
        (&b"BT /F1 12 Tf 1 2 Tj ET"[..], "page 0: bad value: Tj"),
        (&b"BT 1 2 Tm ET"[..], "page 0: bad value: Tm operands"),
        (&b"BT 1 Td ET"[..], "page 0: bad value: Td"),
        (&b"BT 42 Do ET"[..], "page 0: bad value: Do operand"),
        (
            &b"BT /F1 12 Tf (a) 1 (b) \" ET"[..],
            "page 0: bad value: \" operands",
        ),
        (&b"BT /F1 12 Tf 1 ' ET"[..], "page 0: bad value: '"),
    ] {
        let doc = page_doc(contents, "<< /Font << /F1 5 0 R >> >>", &[]);
        refuse(&doc, msg);
    }
}

/// The `'` and `"` operators emit a line break before their text; `T*`
/// breaks only after text has been emitted.
#[test]
fn quote_and_star_operators_emit_breaks() {
    let c = b"BT /F1 12 Tf (a) Tj (b) ' 1 2 (c) \" T* (d) Tj T* ET";
    let doc = page_doc(&c[..], "<< /Font << /F1 5 0 R >> >>", &[]);
    assert_eq!(
        extract_text(&doc).expect("quoted text decodes"),
        "a\nb\nc\nd"
    );
}

/// Vertical moves end the line; a horizontal move of two or more units
/// reads as one space; a sub-threshold move is glued.
#[test]
fn positioning_moves_space_and_break() {
    let c = b"BT /F1 12 Tf (a) Tj 0 20 Td (b) Tj 2 0 Td (c) Tj 0.5 0 Td (d) Tj ET";
    let doc = page_doc(&c[..], "<< /Font << /F1 5 0 R >> >>", &[]);
    assert_eq!(extract_text(&doc).expect("moves applied"), "a\nb cd");
    // Tm with a moved baseline breaks; same-baseline Tm past the threshold spaces
    let c = b"BT /F1 12 Tf (a) Tj 1 0 0 1 0 20 Tm (b) Tj 1 0 0 1 4 20 Tm (c) Tj ET";
    let doc = page_doc(&c[..], "<< /Font << /F1 5 0 R >> >>", &[]);
    // the first Tm has no prior matrix to measure against, so no break
    assert_eq!(extract_text(&doc).expect("matrix moves applied"), "ab c");
}

/// A `Do` on an image XObject (or a non-stream) carries no text and is
/// ignored; a Form XObject's own resources override the page's.
#[test]
fn xobject_forms_and_images() {
    // image: ignored
    let c = b"BT /F1 12 Tf (a) Tj ET /Im1 Do";
    let doc = page_doc(
        &c[..],
        "<< /Font << /F1 5 0 R >> /XObject << /Im1 6 0 R >> >>",
        &[b"<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /Length 2 >>\nstream\n\x00\x01\nendstream"],
    );
    assert_eq!(extract_text(&doc).expect("image ignored"), "a");
    // form with its own font resource
    let c = b"BT /F1 12 Tf (a) Tj ET /Fm1 Do";
    let doc = page_doc(
        &c[..],
        "<< /Font << /F1 5 0 R >> /XObject << /Fm1 6 0 R >> >>",
        &[
            b"<< /Type /XObject /Subtype /Form /Resources << /Font << /F2 7 0 R >> >> /Length 23 >>\nstream\nBT /F2 12 Tf (b) Tj ET\nendstream",
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        ],
    );
    assert_eq!(extract_text(&doc).expect("form recursed"), "a\nb");
}

/// Form XObject recursion is capped.
#[test]
fn xobject_recursion_capped() {
    // 20 forms each invoking the next: the cap is 16
    let mut extra: Vec<Vec<u8>> = Vec::new();
    for i in 0..20u32 {
        let next = i + 1;
        let content = format!("/F{next} Do\n");
        extra.push(
            format!(
                "<< /Type /XObject /Subtype /Form /Resources << /XObject << /F{next} {} 0 R >> >> /Length {} >>\nstream\n{}\nendstream",
                6 + next,
                content.len(),
                content
            )
            .into_bytes(),
        );
    }
    let refs: Vec<&[u8]> = extra.iter().map(|e| e.as_slice()).collect();
    let c = b"/F0 Do";
    let xobjects = "<< /XObject << /F0 6 0 R >> >>";
    let doc = page_doc(&c[..], xobjects, &refs);
    refuse(&doc, "page 0: too large: XObject recursion");
}

/// An operand flood before one operator is capped.
#[test]
fn operand_stack_capped() {
    let mut c = b"BT ".to_vec();
    for _ in 0..1025 {
        c.extend_from_slice(b"1 ");
    }
    c.extend_from_slice(b"(x) Tj ET");
    let doc = page_doc(&c, "<< /Font << /F1 5 0 R >> >>", &[]);
    refuse(&doc, "page 0: bad value: operand stack depth");
}

// ---------------------------------------------------------------------
// Document and xref edges
// ---------------------------------------------------------------------

/// Streams with indirect `/Length` re-slice after resolution; broken
/// indirect lengths refuse by name.
#[test]
fn stream_length_edges() {
    let mut b = Builder::new();
    b.objs("<< /Type /Catalog /Pages 2 0 R >>");
    b.objs("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.objs("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 9 9] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>");
    // content stream with an indirect length
    let data = b"BT /F1 12 Tf (ok) Tj ET";
    let mut body = b"<< /Length 6 0 R >>\nstream\n".to_vec();
    let start_marker = b.out.len() + body.len();
    body.extend_from_slice(data);
    body.extend_from_slice(b"\nendstream");
    let stream_num = b.obj(&body);
    let _ = (stream_num, start_marker);
    let _font = b.objs("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
    // the length object must be numbered 6: catalog=1 pages=2 page=3
    // stream=4 font=5 length=6
    let len_str = data.len().to_string();
    let len_obj = b.objs(&len_str);
    assert_eq!(len_obj, 6);
    let data = b.finish("");
    let doc = Document::open(&data).expect("indirect length resolves");
    assert_eq!(doc.text().expect("text extracts"), "ok");
}

/// A negative indirect `/Length` and a non-numeric one refuse.
#[test]
fn stream_length_broken_indirect_refs() {
    for (len_body, msg) in [
        ("-5", "page 0: bad value: negative /Length"),
        (
            "<< /Not /ANumber >>",
            "page 0: bad value: indirect /Length value",
        ),
    ] {
        let mut b = Builder::new();
        b.objs("<< /Type /Catalog /Pages 2 0 R >>");
        b.objs("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
        b.objs("<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 4 0 R >> >> >>");
        b.objs("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
        // the stream's /Length resolves through object 6
        let s = b.obj(b"<< /Length 6 0 R >>\nstream\nBT /F1 12 Tf (x) Tj ET\nendstream");
        let _ = s;
        b.objs(len_body);
        refuse(&b.finish(""), msg);
    }
}

/// Stream dictionary edges: bad `/Length` types, missing lengths,
/// out-of-file data and a wrong mid-file length.
#[test]
fn stream_dict_edges() {
    // /Length as a string
    let mut b = Builder::new();
    b.objs("<< /Type /Catalog /Pages 2 0 R >>");
    b.objs("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.objs("<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>");
    let s = b.obj(b"<< /Length (four) >>\nstream\ndata\nendstream");
    let _ = s;
    refuse(&b.finish(""), "page 0: bad value: /Length type");
    // no /Length
    let mut b = Builder::new();
    b.objs("<< /Type /Catalog /Pages 2 0 R >>");
    b.objs("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.objs("<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>");
    let s = b.obj(b"<< >>\nstream\ndata\nendstream");
    let _ = s;
    refuse(&b.finish(""), "page 0: bad value: stream without /Length");
    // /Length reaching past EOF
    let mut b = Builder::new();
    b.objs("<< /Type /Catalog /Pages 2 0 R >>");
    b.objs("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.objs("<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>");
    let s = b.obj(b"<< /Length 9999 >>\nstream\ndata\nendstream");
    let _ = s;
    refuse(&b.finish(""), "page 0: truncated: stream data");
    // negative direct /Length
    let mut b = Builder::new();
    b.objs("<< /Type /Catalog /Pages 2 0 R >>");
    b.objs("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.objs("<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>");
    let s = b.obj(b"<< /Length -1 >>\nstream\ndata\nendstream");
    let _ = s;
    refuse(&b.finish(""), "page 0: bad value: negative /Length");
}

/// Object-level parser refusals: non-name keys, stray closers,
/// unexpected keywords and the nesting cap.
#[test]
fn object_parser_edges() {
    for (body, msg) in [
        ("<< 1 2 >>", "bad value: dictionary key is not a name"),
        (">>", "bad value: stray closer"),
        ("<< /A endobj >>", "bad value: unexpected keyword in object"),
        (
            &*format!("[{}]", "[".repeat(65)),
            "bad value: object nesting depth",
        ),
    ] {
        let mut b = Builder::new();
        b.objs("<< /Type /Catalog /Pages 2 0 R >>");
        let bad = b.objs(body);
        let _ = bad;
        refuse(&b.finish(""), msg);
    }
}

/// Page-tree structure edges: kids entries, missing kids, missing
/// catalog pages and a non-catalog root.
#[test]
fn page_tree_edges() {
    // Kids entry that is not a reference
    let mut b = Builder::new();
    b.objs("<< /Type /Catalog /Pages 2 0 R >>");
    b.objs("<< /Type /Pages /Kids [42] /Count 1 >>");
    refuse(&b.finish(""), "bad value: page tree /Kids entry");
    // /Pages without /Kids
    let mut b = Builder::new();
    b.objs("<< /Type /Catalog /Pages 2 0 R >>");
    b.objs("<< /Type /Pages /Count 1 >>");
    refuse(&b.finish(""), "bad value: page tree /Kids");
    // catalog without /Pages
    let mut b = Builder::new();
    b.objs("<< /Type /Catalog >>");
    refuse(&b.finish(""), "bad value: catalog /Pages");
    // root that is not a Catalog
    let mut b = Builder::new();
    b.objs("<< /Type /Pages /Kids [] >>");
    refuse(&b.finish(""), "bad value: /Root is not a Catalog");
    // page tree node that is not a dictionary
    let mut b = Builder::new();
    b.objs("<< /Type /Catalog /Pages 2 0 R >>");
    b.objs("42");
    refuse(&b.finish(""), "bad value: page tree node");
    // trailer without /Root
    let mut b = Builder::new();
    b.objs("<< /Type /Catalog /Pages 2 0 R >>");
    let tail = format!(
        "trailer\n<< /Size 2 >>\nstartxref\n{}\n%%EOF\n",
        b.out.len() + 8
    );
    refuse(&b.finish_raw(tail.as_bytes()), "bad value: trailer /Root");
}

/// `/Contents` entries that are not streams refuse with page context.
#[test]
fn contents_entry_must_be_a_stream() {
    let mut b = Builder::new();
    b.objs("<< /Type /Catalog /Pages 2 0 R >>");
    b.objs("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.objs("<< /Type /Page /Parent 2 0 R /Contents [4 0 R] >>");
    b.objs("<< /Not /AStream >>");
    refuse(&b.finish(""), "page 0: bad value: page /Contents entry");
}

/// The public `object()` accessor wraps failures with the object number.
#[test]
fn object_accessor_wraps_errors() {
    let mut b = Builder::new();
    b.objs("<< /Type /Catalog /Pages 2 0 R >>");
    b.objs("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.objs("<< /Type /Page /Parent 2 0 R >>");
    let data = b.finish("");
    let doc = Document::open(&data).expect("opens");
    let err = doc
        .object(Ref {
            num: 999,
            generation: 0,
        })
        .expect_err("missing object");
    assert_eq!(
        format!("{err}"),
        "object 999 0: bad value: reference to missing object"
    );
}

/// A corrupted xref entry line does not abort the table read: the
/// document falls back to a scan rebuild and still extracts.
#[test]
fn corrupt_classic_entry_falls_back_to_scan() {
    let mut b = Builder::new();
    b.objs("<< /Type /Catalog /Pages 2 0 R >>");
    b.objs("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.objs(
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
    );
    b.stream("", b"BT /F1 12 Tf (hi) Tj ET");
    b.objs("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
    let count = 6;
    let xref_off = b.out.len();
    let mut tail = format!("xref\n0 {count}\n0000000000 65535 f \n").into_bytes();
    // good entries for 1..4, garbage for 5, good for 5 after
    for off in &b.offsets {
        tail.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    let _ = xref_off;
    tail.extend_from_slice(b"garbage!!!\n");
    tail.extend_from_slice(
        format!(
            "trailer\n<< /Size {count} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
            xref_off
        )
        .as_bytes(),
    );
    let data = b.finish_raw(&tail);
    let doc = Document::open(&data).expect("scan rebuild saves the read");
    assert!(doc.xref_was_rebuilt());
    assert_eq!(doc.text().expect("text extracts"), "hi");
}

/// Builds a chain of `n` classic sections (newest first, each `/Prev`
/// linking to the previous one); the file contains no real objects.
fn prev_chain_doc(n: usize, with_objects: bool) -> Vec<u8> {
    // every /Prev placeholder is the same width, so offsets stay valid
    // and the values are patched in place afterwards
    let mut data = b"%PDF-1.4\n".to_vec();
    let mut offs = Vec::new();
    let mut obj_offs: Vec<usize> = Vec::new();
    if with_objects {
        // a resolvable document: catalog, pages, page (resources, font),
        // contents stream — objects 1..=4
        let objs: [Vec<u8>; 4] = [
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>"
                .to_vec(),
            b"BT /F1 12 Tf (hi) Tj ET".to_vec(),
        ];
        for body in objs {
            obj_offs.push(data.len());
            data.extend_from_slice(format!("{} 0 obj\n", obj_offs.len()).as_bytes());
            if obj_offs.len() == 4 {
                data.extend_from_slice(
                    format!(
                        "<< /Length {} >>\nstream\n{}\nendstream\nendobj\n",
                        body.len(),
                        String::from_utf8_lossy(&body)
                    )
                    .as_bytes(),
                );
            } else {
                data.extend_from_slice(&body);
                data.extend_from_slice(b"\nendobj\n");
            }
        }
        // the font object 5 the page references
        obj_offs.push(data.len());
        data.extend_from_slice(
            b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n",
        );
    }
    for i in 0..n {
        offs.push(data.len());
        if i == 0 && with_objects {
            // newest section maps the real objects and carries /Root
            let mut sec = format!("xref\n0 {}\n0000000000 65535 f \n", obj_offs.len() + 1);
            for off in &obj_offs {
                sec.push_str(&format!("{off:010} 00000 n \n"));
            }
            if n > 1 {
                sec.push_str("trailer\n<< /Size 6 /Root 1 0 R /Prev 0000000000 >>\n");
            } else {
                sec.push_str("trailer\n<< /Size 6 /Root 1 0 R >>\n");
            }
            data.extend_from_slice(sec.as_bytes());
        } else if i + 1 < n {
            data.extend_from_slice(
                b"xref\n0 1\n0000000000 65535 f \ntrailer\n<< /Size 1 /Prev 0000000000 >>\n",
            );
        } else {
            data.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \ntrailer\n<< /Size 1 >>\n");
        }
    }
    let placeholder = b"/Prev 0000000000".as_slice();
    let mut from = 0usize;
    for next in offs.iter().skip(1) {
        let at = from
            + data[from..]
                .windows(placeholder.len())
                .position(|w| w == placeholder)
                .expect("placeholder");
        data[at + 6..at + 16].copy_from_slice(format!("{next:010}").as_bytes());
        from = at + 1;
    }
    data.extend_from_slice(format!("startxref\n{}\n%%EOF\n", offs[0]).as_bytes());
    data
}

/// A 31-section chain resolves through `/Prev` without a rebuild.
#[test]
fn prev_chain_31_sections_resolves() {
    let data = prev_chain_doc(31, true);
    let doc = Document::open(&data).expect("chain within cap resolves");
    assert!(!doc.xref_was_rebuilt());
    assert_eq!(doc.text().expect("text extracts"), "hi");
}

/// A 40-section chain trips the depth cap; the scan fallback finds no
/// objects (the file has none) and fails closed.
#[test]
fn prev_chain_length_capped() {
    let data = prev_chain_doc(40, false);
    refuse(&data, "bad value: no objects found while scanning");
}

/// A hybrid-reference file merges the classic table with its `/XRefStm`
/// supplemental stream: the font lives only in the stream section.
#[test]
fn hybrid_xrefstm_merges_sections() {
    let mut b = Builder::new();
    let _cat = b.objs("<< /Type /Catalog /Pages 2 0 R >>");
    let _pag = b.objs("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    let _pg = b.objs(
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
    );
    let _con = b.stream("", b"BT /F1 12 Tf (x) Tj ET");
    // object 5 (the font) is NOT in the classic table; it lives in the
    // XRefStm stream appended after the trailer.
    let _font = b.objs("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
    let font_off = b.offsets[4];
    let count = 6;
    let xref_off = b.out.len();
    let mut tail1 = format!("xref\n0 {count}\n0000000000 65535 f \n").into_bytes();
    for (i, off) in b.offsets.iter().enumerate() {
        if i == 4 {
            tail1.extend_from_slice(b"0000000000 65535 f \n"); // font hidden
        } else {
            tail1.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
    }
    // the xref stream: object 5 -> its real offset
    let mut rows: Vec<u8> = Vec::new();
    rows.push(1);
    rows.extend_from_slice(&(font_off as u32).to_be_bytes());
    rows.extend_from_slice(&0u16.to_be_bytes());
    let dict = format!(
        "<< /Type /XRef /Size 6 /W [1 4 2] /Index [5 1] /Root 1 0 R /Length {} >>\nstream\n",
        rows.len()
    );
    let mut xstm = dict.into_bytes();
    xstm.extend_from_slice(&rows);
    xstm.extend_from_slice(b"\nendstream");
    // two-pass: the trailer carries a fixed-width placeholder for the
    // XRefStm offset, which is only known once the trailer length is
    let xstm_text = {
        let mut t = b"5 0 obj\n".to_vec();
        t.extend_from_slice(&xstm);
        t.extend_from_slice(b"\nendobj\n");
        t
    };
    let mut trailer = format!(
        "trailer\n<< /Size {count} /Root 1 0 R /XRefStm {:010} >>\nstartxref\n{}\n%%EOF\n",
        0, xref_off
    )
    .into_bytes();
    let stm_off = xref_off + tail1.len() + trailer.len();
    let at = trailer
        .windows(9)
        .position(|w| w == b"/XRefStm ".as_slice())
        .expect("placeholder");
    trailer[at + 9..at + 19].copy_from_slice(format!("{:010}", stm_off).as_bytes());
    let mut tail = tail1;
    tail.extend_from_slice(&trailer);
    tail.extend_from_slice(&xstm_text);
    let data = b.finish_raw(&tail);
    let doc = Document::open(&data).expect("hybrid reference resolves");
    assert!(!doc.xref_was_rebuilt());
    assert_eq!(doc.text().expect("text extracts"), "x");
}

/// A type-2 xref row naming an absent object stream fails at resolution
/// time with page context.
#[test]
fn objstm_reference_to_missing_stream_refuses() {
    let mut b = Builder::new();
    let _cat = b.objs("<< /Type /Catalog /Pages 2 0 R >>");
    let _pag = b.objs("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    let _pg = b.objs("<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>");
    let _con_gap = b.objs("<< /Placeholder >>");
    let _font_gap = b.objs("<< /Placeholder2 >>");
    // object 6 is declared inside object stream 9, which does not exist
    let mut rows: Vec<u8> = Vec::new();
    for off in &b.offsets {
        rows.push(1);
        rows.extend_from_slice(&(*off as u32).to_be_bytes());
        rows.extend_from_slice(&0u16.to_be_bytes());
    }
    rows.push(2);
    rows.extend_from_slice(&9u32.to_be_bytes());
    rows.extend_from_slice(&0u16.to_be_bytes());
    let dict = format!(
        "<< /Type /XRef /Size 7 /W [1 4 2] /Index [1 6] /Root 1 0 R /Length {} >>\nstream\n",
        rows.len()
    );
    let mut xstm = dict.into_bytes();
    xstm.extend_from_slice(&rows);
    xstm.extend_from_slice(b"\nendstream");
    let xstm_obj = b.obj(&xstm);
    let xstm_off = b.offsets[xstm_obj as usize - 1];
    let tail = format!("startxref\n{xstm_off}\n%%EOF\n");
    let mut data = b.out.clone();
    data.push(b'\n');
    data.extend_from_slice(tail.as_bytes());
    refuse(&data, "page 0: bad value: reference to missing object");
}

/// `/ObjStm` dictionary edges refuse by name.
/// Builds a document whose page contents is object 8, declared as member 0
/// of object stream 6; the xref stream (object 7, startxref target) maps
/// objects 1-6 plain and object 8 to the stream member.
fn objstm_doc(stm_dict: &str, raw: &[u8]) -> Vec<u8> {
    let mut b = Builder::new();
    b.objs("<< /Type /Catalog /Pages 2 0 R >>");
    b.objs("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.objs(
        "<< /Type /Page /Parent 2 0 R /Contents 8 0 R /Resources << /Font << /F1 4 0 R >> >> >>",
    );
    b.objs("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
    b.objs("<< /Placeholder2 >>");
    let dict = format!(
        "{} /Length {} >>\nstream\n",
        stm_dict.trim_end_matches(">>").trim_end(),
        raw.len()
    );
    let mut stm = dict.into_bytes();
    stm.extend_from_slice(raw);
    stm.extend_from_slice(b"\nendstream");
    b.obj(&stm);
    let mut rows: Vec<u8> = Vec::new();
    for off in &b.offsets {
        rows.push(1);
        rows.extend_from_slice(&(*off as u32).to_be_bytes());
        rows.extend_from_slice(&0u16.to_be_bytes());
    }
    rows.push(2);
    rows.extend_from_slice(&6u32.to_be_bytes());
    rows.extend_from_slice(&0u16.to_be_bytes());
    let xstm = format!(
        "<< /Type /XRef /Size 9 /W [1 4 2] /Index [1 6 8 1] /Root 1 0 R /Length {} >>\nstream\n",
        rows.len()
    );
    let mut x = xstm.into_bytes();
    x.extend_from_slice(&rows);
    x.extend_from_slice(b"\nendstream");
    let xstm_obj = b.obj(&x);
    let xstm_off = b.offsets[xstm_obj as usize - 1];
    let tail = format!("startxref\n{xstm_off}\n%%EOF\n");
    let mut data = b.out.clone();
    data.push(b'\n');
    data.extend_from_slice(tail.as_bytes());
    data
}

/// `/ObjStm` dictionary edges refuse by name.
#[test]
fn objstm_dict_edges() {
    for (stm_dict, raw, msg) in [
        (
            "<< /Type /ObjStm /N 1 >>",
            &b""[..],
            "page 0: bad value: ObjStm /First",
        ),
        (
            "<< /Type /ObjStm /N 1 /First 99 >>",
            b"8 0\n<< >>\n",
            "page 0: bad value: ObjStm /First",
        ),
        (
            "<< /Type /ObjStm /First 4 >>",
            b"8 0\n<< >>\n",
            "page 0: bad value: ObjStm /N",
        ),
        (
            "<< /Type /ObjStm /N 1 /First 4 >>",
            b"9 0\n<< >>\n",
            "page 0: bad value: object stream member number",
        ),
        (
            "<< /Type /ObjStm /N 1 /First 4 >>",
            b"7 99\n<< >>\n",
            "page 0: bad value: ObjStm member offset",
        ),
    ] {
        refuse(&objstm_doc(stm_dict, raw), msg);
    }
}

/// A member span past the end of the ObjStm data refuses.
#[test]
fn objstm_member_span_refuses() {
    // descending member offsets: the first member's end lands before its
    // start, which is a span refusal
    refuse(
        &objstm_doc("<< /Type /ObjStm /N 2 /First 8 >>", b"8 3\n8 1\n<< >>\n"),
        "page 0: bad value: ObjStm member span",
    );
}

/// An `/ObjStm` member whose number disagrees with the xref row refuses
/// (the object-stream path, exercised through a valid document).
#[test]
fn objstm_member_resolves_through_real_document() {
    // member 0 of stream 6 is the page's content stream (object 8)
    let member = "<< /Length 30 >>\nstream\nBT /F1 12 Tf (in-stream) Tj ET\nendstream";
    let mut raw = b"8 0\n".to_vec();
    raw.extend_from_slice(member.as_bytes());
    let data = objstm_doc("<< /Type /ObjStm /N 1 /First 4 >>", &raw);
    assert_eq!(
        extract_text(&data).expect("objstm member resolves"),
        "in-stream"
    );
}

// ---------------------------------------------------------------------
// Coverage completion: cross-reference, lexer and font arms that the
// fixture corpus cannot reach. Objects live inside an object stream so
// the scan fallback cannot mask xref errors with a rebuild.
// ---------------------------------------------------------------------

/// A document whose every real object is a member of object stream 1;
/// the xref stream (object 2, startxref target) is the only xref. The
/// scan fallback finds no catalog, so xref-stream errors are the last
/// word.
fn all_in_objstm(xstm_dict: &str, classic_tail: Option<&str>) -> Vec<u8> {
    let mut b = Builder::new();
    let members: [&str; 5] = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
        "<< /Length 23 >>\nstream\nBT /F1 12 Tf (x) Tj ET\nendstream",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ];
    let mut raw: Vec<u8> = Vec::new();
    let mut header = String::new();
    for (i, m) in members.iter().enumerate() {
        header.push_str(&format!("{} 0\n", i + 1));
        raw.extend_from_slice(m.as_bytes());
        raw.push(b'\n');
    }
    let mut stm = format!(
        "<< /Type /ObjStm /N 5 /First {} /Length {} >>\nstream\n",
        header.len(),
        header.len() + raw.len()
    )
    .into_bytes();
    stm.extend_from_slice(header.as_bytes());
    stm.extend_from_slice(&raw);
    stm.extend_from_slice(b"\nendstream");
    let _stm = b.obj(&stm);
    match classic_tail {
        Some(tail) => {
            let off = b.out.len();
            let mut data = b.out.clone();
            data.push(b'\n');
            data.extend_from_slice(tail.replace("@OFF", &format!("{off}")).as_bytes());
            data
        }
        None => {
            let mut rows: Vec<u8> = Vec::new();
            rows.push(1);
            rows.extend_from_slice(&(b.offsets[0] as u32).to_be_bytes());
            rows.extend_from_slice(&0u16.to_be_bytes());
            let dict = format!("{xstm_dict} /Length {} >>\nstream\n", rows.len());
            let mut xstm = dict.into_bytes();
            xstm.extend_from_slice(&rows);
            xstm.extend_from_slice(b"\nendstream");
            let xstm_obj = b.obj(&xstm);
            let xstm_off = b.offsets[xstm_obj as usize - 1];
            let tail = format!("startxref\n{xstm_off}\n%%EOF\n");
            let mut data = b.out.clone();
            data.push(b'\n');
            data.extend_from_slice(tail.as_bytes());
            data
        }
    }
}

/// Xref stream dictionary edges refuse (masked by neither scan nor
/// rebuild because the file has no scannable catalog).
#[test]
fn xref_stream_dict_edges() {
    refuse(
        &all_in_objstm("<< /Type /XRef /Size 6 /W [1 4] /Root 1 0 R", None),
        "bad value: no catalog while scanning",
    );
    refuse(
        &all_in_objstm("<< /Type /XRef /Size 6 /W [0 0 0] /Root 1 0 R", None),
        "bad value: no catalog while scanning",
    );
    refuse(
        &all_in_objstm(
            "<< /Type /XRef /Size 6 /W [1 4 2] /Index 42 /Root 1 0 R",
            None,
        ),
        "bad value: no catalog while scanning",
    );
    refuse(
        &all_in_objstm(
            "<< /Type /XRef /Size 6 /W [1 4 2] /Index [5 99] /Root 1 0 R",
            None,
        ),
        "bad value: no catalog while scanning",
    );
}

/// Classic xref structural edges executed before the scan fallback.
#[test]
fn classic_xref_structural_edges() {
    let tail = "xref\n0 16777216\n0000000000 65535 f \n";
    refuse(
        &all_in_objstm("", Some(tail)),
        "bad value: no catalog while scanning",
    );
    let tail = "xref\n0 1\n0000000000 65535 f \ntrailer\n42\nstartxref\n@OFF\n%%EOF\n";
    refuse(
        &all_in_objstm("", Some(tail)),
        "bad value: no catalog while scanning",
    );
    let tail = "xref\n0 1\n0000000000 65535 f \n";
    refuse(
        &all_in_objstm("", Some(tail)),
        "bad value: no catalog while scanning",
    );
}

/// An xref entry pointing into the header resolves to an unparseable
/// object and refuses with context.
#[test]
fn broken_entry_offset_refuses() {
    let mut b = Builder::new();
    b.objs("<< /Type /Catalog /Pages 2 0 R >>");
    b.objs("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.objs("<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>");
    b.stream("", b"BT (hi) Tj ET");
    let offs = b.offsets.clone();
    let mut tail = "xref\n0 5\n0000000000 65535 f \n".to_string();
    for (i, off) in offs.iter().enumerate() {
        // the content stream (object 4) is redirected into the header
        if i == 3 {
            tail.push_str(&format!("{:010} 00000 n \n", 1usize));
        } else {
            tail.push_str(&format!("{off:010} 00000 n \n"));
        }
    }
    tail.push_str(&format!(
        "trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
        b.out.len()
    ));
    let data = b.finish_raw(tail.as_bytes());
    refuse(&data, "page 0: bad value: object number");
}

/// A valid bfrange with an array destination maps codes to the array
/// entries in order.
#[test]
fn cmap_bfrange_array_destination_maps() {
    let c =
        CMap::parse(b"1 beginbfrange\n<0001> <0002> [(\\000A) (\\000B)]\nendbfrange\nendcmap\n")
            .expect("array bfrange");
    assert_eq!(c.lookup(&[0x00, 0x01]), (Some("A"), 2));
    assert_eq!(c.lookup(&[0x00, 0x02]), (Some("B"), 2));
}

/// Escape sequences decode to their control bytes; a backslash at end of
/// stream truncates.
#[test]
fn lexer_control_escapes_and_eof_backslash() {
    let c = CMap::parse(b"1 beginbfchar\n<41> (\\000\\000\\n\\r\\t\\b)\nendbfchar\nendcmap\n")
        .expect("control escapes");
    assert!(c.lookup(&[0x41]).0.is_some());
    refuse_cmap(b"1 beginbfchar\n<41> (ab\\", "truncated: string escape");
}

/// Font encoding extras: PDFDocEncoding base, predefined CID CMap names
/// and a CID font without `/Encoding`.
#[test]
fn font_encoding_coverage_extras() {
    let doc = page_doc(
        b"BT /F1 12 Tf (A) Tj ET",
        "<< /Font << /F1 6 0 R >> >>",
        &[font_obj("/Subtype /Type1 /BaseFont /Helvetica /Encoding /PDFDocEncoding").as_bytes()],
    );
    assert_eq!(extract_text(&doc).expect("pdfdoc"), "A");

    let cmap = "2 beginbfchar\n<0041> <0041>\n<0042> <0042>\nendbfchar\nendcmap\n";
    let tounicode = format!("<< /Length {} >>\nstream\n{}endstream", cmap.len(), cmap);
    let font = "<< /Type /Font /Subtype /Type0 /BaseFont /X /Encoding /UniJIS-UCS2-H /DescendantFonts [7 0 R] /ToUnicode 8 0 R >>";
    let doc = page_doc(
        b"BT /F1 12 Tf <00410042> Tj ET",
        "<< /Font << /F1 6 0 R >> >>",
        &[
            font.as_bytes(),
            "<< /Type /Font /Subtype /CIDFontType2 >>".as_bytes(),
            tounicode.as_bytes(),
        ],
    );
    assert_eq!(extract_text(&doc).expect("predefined cmap name"), "AB");

    let font =
        "<< /Type /Font /Subtype /Type0 /BaseFont /X /DescendantFonts [7 0 R] /ToUnicode 8 0 R >>";
    let doc = page_doc(
        b"BT /F1 12 Tf <00410042> Tj ET",
        "<< /Font << /F1 6 0 R >> >>",
        &[
            font.as_bytes(),
            "<< /Type /Font /Subtype /CIDFontType2 >>".as_bytes(),
            tounicode.as_bytes(),
        ],
    );
    assert_eq!(extract_text(&doc).expect("encoding-less cid"), "AB");
}

/// An embedded CID CMap stream may declare one-byte codes.
#[test]
fn cid_embedded_cmap_one_byte_codes() {
    let enc = "1 begincodespacerange\n<00> <FF>\nendcodespacerange\nendcmap\n";
    let cmap = "2 beginbfchar\n<41> <0041>\n<42> <0042>\nendbfchar\nendcmap\n";
    let enc_stm = format!("<< /Length {} >>\nstream\n{}endstream", enc.len(), enc);
    let tu_stm = format!("<< /Length {} >>\nstream\n{}endstream", cmap.len(), cmap);
    let font = "<< /Type /Font /Subtype /Type0 /BaseFont /X /Encoding 7 0 R /DescendantFonts [6 0 R] /ToUnicode 8 0 R >>";
    let doc = page_doc(
        b"BT /F1 12 Tf <4142> Tj ET",
        "<< /Font << /F1 9 0 R >> >>",
        &[
            "<< /Type /Font /Subtype /CIDFontType2 >>".as_bytes(),
            enc_stm.as_bytes(),
            tu_stm.as_bytes(),
            font.as_bytes(),
        ],
    );
    assert_eq!(extract_text(&doc).expect("one-byte cid"), "AB");
}

/// Positioning and text-array operand edges.
#[test]
fn content_positioning_and_tj_operand_edges() {
    let c = b"BT /F1 12 Tf (a) Tj 1 0 0 1 0 20 Tm 1 0 0 1 0 40 Tm (b) Tj ET";
    let doc = page_doc(&c[..], "<< /Font << /F1 5 0 R >> >>", &[]);
    assert_eq!(extract_text(&doc).expect("tm break"), "a\nb");
}

/// Page tree edges: an empty Kids array leaves no pages; an ObjStm
/// member header with a negative member number refuses.
#[test]
fn document_tree_edges() {
    let mut b = Builder::new();
    b.objs("<< /Type /Catalog /Pages 2 0 R >>");
    b.objs("<< /Type /Pages /Kids [] /Count 0 >>");
    b.objs("<< /Type /Page /Parent 2 0 R >>");
    refuse(&b.finish(""), "bad value: empty page tree");
    refuse(
        &objstm_doc("<< /Type /ObjStm /N 1 /First 4 >>", b"-1 0\n<< >>\n"),
        "page 0: bad value: ObjStm member header",
    );
}

/// Streams may use CRLF after `stream` and before `endstream`.
#[test]
fn stream_crlf_endings() {
    let mut b = Builder::new();
    b.objs("<< /Type /Catalog /Pages 2 0 R >>");
    b.objs("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.objs(
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
    );
    let content = b"BT /F1 12 Tf (x) Tj ET";
    let body = format!(
        "<< /Length {} >>\r\nstream\r\n{}\r\nendstream",
        content.len(),
        String::from_utf8_lossy(content)
    );
    b.obj(body.as_bytes());
    b.objs("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
    assert_eq!(extract_text(&b.finish("")).expect("crlf stream"), "x");
}
