//! The C ABI surface of `pith-pdf`: the entry points the Python
//! (ctypes), Node (koffi) and Go (cgo) SDKs bind through.
//!
//! The suite's FFI convention, defined by this module and mirrored by
//! every `pith-*` cdylib:
//!
//! * one flat set of `#[unsafe(no_mangle)] pub unsafe extern "C"`
//!   functions — raw pointers plus lengths, no structs across the
//!   boundary;
//! * every function returns a status code (see the constants below),
//!   never a `Result`, never a panic: a `panic = "abort"` cdylib must
//!   not be reachable from a foreign caller;
//! * an operation either hands ownership to the caller (and ships a
//!   matching `_free` — [`pith_pdf_free`] here) or writes into
//!   caller-provided out-parameters;
//! * the `unsafe` allowance is confined to this module; every core
//!   module stays unsafe-free behind the crate-root `#![deny]`.
//!
//! Extraction uses the crate's conservative defaults: [`Document::open`]
//! with [`pith_inflate::Limits::default`] — a hashing pipeline never
//! wants an unbounded decode, and the FFI surface is no exception.

#![allow(unsafe_code)]

use crate::{Document, Error};
use pith_digest::Error as KErr;

/// Status: success.
pub const PITH_OK: i32 = 0;
/// Status: a caller argument is invalid — a null pointer.
pub const PITH_E_INVALID: i32 = -1;
/// Status: the core refused the input (malformed PDF: truncated
/// header, unreadable xref with no scannable fallback, parse error).
pub const PITH_E_REJECTED: i32 = -2;
/// Status: the core refuses to guess — an encrypted document, or any
/// [`KErr::Unsupported`] anywhere in the error chain (an unimplemented
/// stream filter, a CID font without `/ToUnicode`). Distinguishing
/// "malformed" from "deliberately not implemented" is why this is not
/// the uniform [`PITH_E_REJECTED`].
pub const PITH_E_UNSUPPORTED: i32 = -3;

/// Extracts all text from a PDF into the canonical byte stream the
/// `reference.json` vectors are defined over.
///
/// `data` points at `len` bytes of the complete PDF file. On success
/// the function allocates a buffer, writes its address through `out`,
/// its length through `out_len`, and returns [`PITH_OK`]; the caller
/// owns the buffer and must release it with [`pith_pdf_free`], passing
/// back the same pointer *and* length. The buffer layout is the
/// canonical extraction serialization: `pages` (`u32` big-endian),
/// `rebuilt` (`u8`, 0/1 — whether the xref was rebuilt by scanning),
/// `text_len` (`u64` big-endian), then the extracted text as UTF-8 —
/// exactly the bytes the vectors' `text_sha256` covers.
///
/// # Safety
///
/// `data` must point to `len` readable bytes; `out` to one writable
/// pointer; `out_len` to one writable `usize`. All must stay valid for
/// the duration of the call; the function retains nothing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pith_pdf_extract_canonical(
    data: *const u8,
    len: usize,
    out: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    if data.is_null() || out.is_null() || out_len.is_null() {
        return PITH_E_INVALID;
    }
    let bytes = unsafe { core::slice::from_raw_parts(data, len) };
    match extract_and_serialize(bytes) {
        Ok(canonical) => {
            let len = canonical.len();
            // Hand the exact-length buffer to the caller; `pith_pdf_free`
            // reconstructs the boxed slice from the same length.
            let ptr = alloc::boxed::Box::into_raw(canonical.into_boxed_slice());
            unsafe {
                *out = ptr.cast::<u8>();
                *out_len = len;
            }
            PITH_OK
        }
        Err(status) => status,
    }
}

/// Releases a buffer handed out by [`pith_pdf_extract_canonical`].
///
/// # Safety
///
/// `ptr` must be a pointer returned by [`pith_pdf_extract_canonical`]
/// with the `out_len` value that came back with it, and must not have
/// been released (or otherwise freed) before. Null is accepted and
/// ignored, so callers can free unconditionally on the error path.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pith_pdf_free(ptr: *mut u8, len: usize) {
    if ptr.is_null() {
        return;
    }
    let slice = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
    drop(unsafe { alloc::boxed::Box::from_raw(slice) });
}

/// Whether [`KErr::Unsupported`] appears anywhere in the error chain:
/// a `Kit` variant carries it directly, an `Object`/`Page` wrapper
/// carries it through its cause.
fn is_unsupported(err: &Error) -> bool {
    match err {
        Error::Kit(KErr::Unsupported(_)) => true,
        Error::Kit(_) => false,
        Error::Object { cause, .. } | Error::Page { cause, .. } => is_unsupported(cause),
    }
}

/// Maps a core [`Error`] onto an FFI status: an unsupported-feature
/// refusal is [`PITH_E_UNSUPPORTED`], every other failure is
/// [`PITH_E_REJECTED`].
fn status_of(err: &Error) -> i32 {
    if is_unsupported(err) {
        PITH_E_UNSUPPORTED
    } else {
        PITH_E_REJECTED
    }
}

/// The safe core of [`pith_pdf_extract_canonical`]: open the document,
/// extract every page, then serialize canonically. Refusals map to
/// [`PITH_E_REJECTED`] or [`PITH_E_UNSUPPORTED`].
fn extract_and_serialize(bytes: &[u8]) -> Result<alloc::vec::Vec<u8>, i32> {
    let doc = Document::open(bytes).map_err(|e| status_of(&e))?;
    let pages = u32::try_from(doc.pages()).unwrap_or(u32::MAX);
    let rebuilt = doc.xref_was_rebuilt();
    let text = doc.text().map_err(|e| status_of(&e))?;
    let mut stream = alloc::vec::Vec::with_capacity(13 + text.len());
    stream.extend_from_slice(&pages.to_be_bytes());
    stream.push(u8::from(rebuilt));
    stream.extend_from_slice(&(text.len() as u64).to_be_bytes());
    stream.extend_from_slice(text.as_bytes());
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::{
        PITH_E_INVALID, PITH_E_REJECTED, PITH_E_UNSUPPORTED, PITH_OK, extract_and_serialize,
        pith_pdf_extract_canonical, pith_pdf_free,
    };
    use std::path::PathBuf;

    /// Reads a committed conformance fixture.
    fn fixture(name: &str) -> alloc::vec::Vec<u8> {
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.push("tests/fixtures");
        p.push(name);
        std::fs::read(&p).expect("fixture")
    }

    /// A committed conformance fixture, extracted end-to-end through the
    /// raw FFI: status OK, the documented wire layout, the pinned page
    /// count and text digest, and the buffer round-trips through
    /// `pith_pdf_free`.
    #[test]
    fn ffi_extract_reproduces_the_canonical_stream() {
        let pdf = fixture("empty_page.pdf");
        let expected = extract_and_serialize(&pdf).expect("extract");

        let mut out: *mut u8 = core::ptr::null_mut();
        let mut out_len: usize = 0;
        let status =
            unsafe { pith_pdf_extract_canonical(pdf.as_ptr(), pdf.len(), &mut out, &mut out_len) };
        assert_eq!(status, PITH_OK);
        assert_eq!(out_len, expected.len());
        let handed_back = unsafe { core::slice::from_raw_parts(out, out_len) };
        assert_eq!(handed_back, expected.as_slice());
        // The first 13 bytes are the documented header: 2 pages, xref
        // intact (not rebuilt), 14 text bytes.
        assert_eq!(
            &handed_back[..13],
            &[0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 14]
        );
        let text = &handed_back[13..];
        assert_eq!(text.len(), 14);
        assert_eq!(
            pith_digest::sha256(text).expect("sha256").to_string(),
            "c8bbf32ef0e7ddc929a2532c9f4137c6c1d3da21896b94f406166fb273e645f7"
        );
        unsafe { pith_pdf_free(out, out_len) };
    }

    /// An encrypted document refuses extraction with
    /// [`PITH_E_UNSUPPORTED`]: a real format feature this suite will
    /// not guess at, not a malformed file.
    #[test]
    fn ffi_encrypted_document_maps_to_unsupported() {
        let pdf = fixture("encrypted.pdf");
        assert_eq!(
            extract_and_serialize(&pdf),
            Err(PITH_E_UNSUPPORTED),
            "the /Encrypt refusal must walk the page/object chain to Unsupported"
        );
        let mut out: *mut u8 = core::ptr::null_mut();
        let mut out_len: usize = 0;
        let status =
            unsafe { pith_pdf_extract_canonical(pdf.as_ptr(), pdf.len(), &mut out, &mut out_len) };
        assert_eq!(status, PITH_E_UNSUPPORTED);
    }

    /// Null pointers are [`PITH_E_INVALID`]; garbage input is
    /// [`PITH_E_REJECTED`]; a null buffer is a legal free.
    #[test]
    fn ffi_refusals() {
        let mut out: *mut u8 = core::ptr::null_mut();
        let mut out_len: usize = 0;
        let null_data =
            unsafe { pith_pdf_extract_canonical(core::ptr::null(), 0, &mut out, &mut out_len) };
        assert_eq!(null_data, PITH_E_INVALID);

        let garbage = [0u8; 16];
        let null_out = unsafe {
            pith_pdf_extract_canonical(
                garbage.as_ptr(),
                garbage.len(),
                core::ptr::null_mut(),
                &mut out_len,
            )
        };
        assert_eq!(null_out, PITH_E_INVALID);
        let null_out_len = unsafe {
            pith_pdf_extract_canonical(
                garbage.as_ptr(),
                garbage.len(),
                &mut out,
                core::ptr::null_mut(),
            )
        };
        assert_eq!(null_out_len, PITH_E_INVALID);

        let rejected = unsafe {
            pith_pdf_extract_canonical(garbage.as_ptr(), garbage.len(), &mut out, &mut out_len)
        };
        assert_eq!(rejected, PITH_E_REJECTED);

        unsafe { pith_pdf_free(core::ptr::null_mut(), 0) };
    }

    /// The whole canonical stream of `empty_page` (header + text) has a
    /// stable digest, pinned here and re-asserted by the Python, Node
    /// and Go SDK test suites; this fails loudly if the wire layout or
    /// the extractor drifts.
    #[test]
    fn canonical_stream_digest_is_stable() {
        let pdf = fixture("empty_page.pdf");
        let canonical = extract_and_serialize(&pdf).expect("extract");
        assert_eq!(
            pith_digest::sha256(&canonical).expect("sha256").to_string(),
            "5a8db37861d2ffa061f87ad2ec1b4c55181e695473ec20dbd228bad355b40e7e"
        );
    }
}
