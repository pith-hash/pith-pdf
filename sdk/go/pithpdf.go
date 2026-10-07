// SPDX-License-Identifier: MIT
// Copyright (c) 2026 pith-hash

// Package pithpdf provides Go bindings for the pith-pdf Rust cdylib:
// PDF text extraction into the canonical vector stream.
//
// The single Rust core (built by `cargo build --release`) is loaded at
// runtime; the package carries zero module dependencies. On unix the
// cdylib is opened with dlopen through cgo, on Windows with
// LoadLibrary through the standard syscall package — both resolve the
// library through the same discovery chain, so `go build ./... &&
// go test ./...` works unchanged on every OS the CD matrix builds.
//
// Discovery order (the suite's cdylib convention):
//
//  1. PITH_CDYLIB — an explicit cdylib file path;
//  2. PITH_CDYLIB_DIR — a directory scanned for the cdylib names (the
//     CD pipeline points this at target/release);
//  3. <repo root>/target/release — the repository working-tree layout,
//     anchored at this package's source directory, so a source
//     checkout runs against a local cargo build unconfigured.
//
// The FFI surface is one extraction operation plus one free:
// pith_pdf_extract_canonical opens a whole PDF document and extracts
// all text into the canonical byte stream the reference.json vectors
// are defined over (page count big-endian, rebuilt flag, text length
// big-endian, UTF-8 text), and pith_pdf_free releases the handed-out
// buffer.
package pithpdf

import (
	"encoding/binary"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"sync"
	"unsafe"
)

// Status codes returned by the cdylib's C ABI.
const (
	// StatusOK: success.
	StatusOK int32 = 0
	// StatusInvalid: a caller argument is invalid (a null pointer).
	StatusInvalid int32 = -1
	// StatusRejected: the core refused the input (malformed PDF:
	// truncated header, unreadable xref, parse error).
	StatusRejected int32 = -2
	// StatusUnsupported: the core refuses to guess — an encrypted
	// document, or an unimplemented format feature (stream filter,
	// CID font without /ToUnicode) anywhere in the error chain.
	StatusUnsupported int32 = -3
)

// cdylibNames are the file names cargo may drop into the build
// directory, per platform (windows / linux / macOS).
var cdylibNames = []string{"pith_pdf.dll", "libpith_pdf.so", "libpith_pdf.dylib"}

// FfiError reports a non-zero status code from the cdylib.
type FfiError struct {
	// Op is the FFI operation name.
	Op string
	// Status is the raw status code the FFI returned.
	Status int32
}

func (e *FfiError) Error() string {
	kind := "unknown failure"
	switch e.Status {
	case StatusInvalid:
		kind = "invalid argument"
	case StatusRejected:
		kind = "input rejected"
	case StatusUnsupported:
		kind = "unsupported feature"
	}
	return fmt.Sprintf("%s failed: %s (status %d)", e.Op, kind, e.Status)
}

// FindCdylib locates the cdylib through the suite's discovery chain.
func FindCdylib() (string, error) {
	if p := os.Getenv("PITH_CDYLIB"); p != "" {
		if st, err := os.Stat(p); err == nil && st.Mode().IsRegular() {
			return filepath.Abs(p)
		}
	}
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		return "", fmt.Errorf("pithpdf: cannot locate the package source directory")
	}
	pkgDir := filepath.Dir(thisFile)
	repoRoot := filepath.Dir(filepath.Dir(pkgDir)) // sdk/go -> sdk -> repo root

	var dirs []string
	if env := os.Getenv("PITH_CDYLIB_DIR"); env != "" {
		dirs = append(dirs, env)
		if !filepath.IsAbs(env) {
			dirs = append(dirs, filepath.Join(repoRoot, env))
		}
	}
	dirs = append(dirs, filepath.Join(repoRoot, "target", "release"))
	for _, dir := range dirs {
		for _, name := range cdylibNames {
			p := filepath.Join(dir, name)
			if st, err := os.Stat(p); err == nil && st.Mode().IsRegular() {
				return p, nil
			}
		}
	}
	return "", fmt.Errorf(
		"pithpdf: no cdylib found (searched PITH_CDYLIB, PITH_CDYLIB_DIR and <repo>/target/release); run `cargo build --release` first",
	)
}

// locate resolves the cdylib path once per process.
var locate = sync.OnceValues(FindCdylib)

// Canonical is the extracted document, re-expressed from the canonical
// byte stream: the page count, the xref-recovery flag and the text.
type Canonical struct {
	// Pages is the page count in document order.
	Pages uint32
	// Rebuilt reports that the xref was rebuilt by scanning (a
	// recovery ran; the result is reported, never silent).
	Rebuilt bool
	// Text is every page's text joined with \x0c (form feed).
	Text string
	// TextBytes is the extracted text as UTF-8 — exactly the bytes
	// the vectors' text_sha256 digest covers.
	TextBytes []byte
}

// ExtractCanonical opens a complete PDF document and extracts all text
// into the canonical byte stream the reference.json vectors are
// defined over. The returned slice is a Go copy; the handed-out cdylib
// buffer is released before returning.
func ExtractCanonical(data []byte) ([]byte, error) {
	libPath, err := locate()
	if err != nil {
		return nil, err
	}
	var out *byte
	var outLen uintptr
	var dataPtr *byte
	if len(data) > 0 {
		dataPtr = &data[0]
	}
	status, err := ffiExtractCanonical(libPath, dataPtr, len(data), &out, &outLen)
	if err != nil {
		return nil, err
	}
	if status != StatusOK {
		return nil, &FfiError{Op: "pith_pdf_extract_canonical", Status: status}
	}
	buf := make([]byte, outLen)
	copy(buf, unsafe.Slice(out, outLen))
	ffiFree(libPath, out, outLen)
	return buf, nil
}

// ParseCanonical re-expresses the canonical byte stream as a Canonical.
func ParseCanonical(raw []byte) (Canonical, error) {
	if len(raw) < 13 {
		return Canonical{}, fmt.Errorf("pithpdf: canonical stream is shorter than the 13-byte header")
	}
	textLen := binary.BigEndian.Uint64(raw[5:13])
	if uint64(len(raw)) < 13+textLen {
		return Canonical{}, fmt.Errorf("pithpdf: canonical stream is shorter than its declared text length")
	}
	text := raw[13 : 13+textLen]
	return Canonical{
		Pages:     binary.BigEndian.Uint32(raw[0:4]),
		Rebuilt:   raw[4] == 1,
		Text:      string(text),
		TextBytes: text,
	}, nil
}
