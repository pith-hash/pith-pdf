// SPDX-License-Identifier: MIT
// Copyright (c) 2026 pith-hash

package pithpdf

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// repoRoot resolves the repository root relative to this package
// (sdk/go -> sdk -> repo root), the anchor for reference.json and the
// committed fixtures.
func repoRoot(t *testing.T) string {
	t.Helper()
	root, err := filepath.Abs(filepath.Join("..", ".."))
	if err != nil {
		t.Fatal(err)
	}
	if st, err := os.Stat(filepath.Join(root, "reference.json")); err != nil || st.IsDir() {
		t.Fatalf("reference.json not found at %s", root)
	}
	return root
}

// reference parses the committed reference.json: the fixture vectors
// (positive, fixture-file driven), the inline-hex recovery vector, and
// the refusal rows grouped by replayed API.
func reference(t *testing.T) (
	fixtures []struct {
		Name     string `json:"name"`
		Pages    uint32 `json:"pages"`
		Rebuilt  bool   `json:"rebuilt"`
		TextByte int    `json:"text_bytes"`
		TextSha  string `json:"text_sha256"`
	},
	encrypted struct {
		Pages     uint32 `json:"pages"`
		Encrypted bool   `json:"encrypted"`
		Error     string `json:"error"`
	},
	recovered struct {
		Name     string `json:"name"`
		Pages    uint32 `json:"pages"`
		Rebuilt  bool   `json:"rebuilt"`
		TextByte int    `json:"text_bytes"`
		TextSha  string `json:"text_sha256"`
		Input    string `json:"input"`
	},
	openRefusals []struct {
		Name  string `json:"name"`
		API   string `json:"api"`
		Input string `json:"input"`
		Error string `json:"error"`
	},
	excludedCount int,
) {
	t.Helper()
	raw, err := os.ReadFile(filepath.Join(repoRoot(t), "reference.json"))
	if err != nil {
		t.Fatal(err)
	}
	var parsed struct {
		Vectors []json.RawMessage `json:"vectors"`
		Errors  []struct {
			Name  string `json:"name"`
			API   string `json:"api"`
			Input string `json:"input"`
			Error string `json:"error"`
		} `json:"errors"`
	}
	if err := json.Unmarshal(raw, &parsed); err != nil {
		t.Fatal(err)
	}
	for _, v := range parsed.Vectors {
		var probe struct {
			Name      string  `json:"name"`
			Input     *string `json:"input"`
			Encrypted *bool   `json:"encrypted"`
			TextSha   *string `json:"text_sha256"`
		}
		if err := json.Unmarshal(v, &probe); err != nil {
			t.Fatal(err)
		}
		switch {
		case probe.Name == "encrypted":
			if err := json.Unmarshal(v, &encrypted); err != nil {
				t.Fatal(err)
			}
		case probe.Input != nil:
			if err := json.Unmarshal(v, &recovered); err != nil {
				t.Fatal(err)
			}
		case probe.TextSha != nil:
			var row struct {
				Name     string `json:"name"`
				Pages    uint32 `json:"pages"`
				Rebuilt  bool   `json:"rebuilt"`
				TextByte int    `json:"text_bytes"`
				TextSha  string `json:"text_sha256"`
			}
			if err := json.Unmarshal(v, &row); err != nil {
				t.Fatal(err)
			}
			fixtures = append(fixtures, row)
		default:
			t.Fatalf("unclassifiable vector row %q", probe.Name)
		}
	}
	for _, e := range parsed.Errors {
		if e.API == "Document::open" {
			openRefusals = append(openRefusals, e)
		} else {
			excludedCount++
		}
	}
	return fixtures, encrypted, recovered, openRefusals, excludedCount
}

// TestFixtureVectorsHexExact replays every fixture vector through the
// cdylib and compares byte-exact: the extracted text's SHA-256 against
// text_sha256, plus the page count, the xref-rebuilt flag and the text
// byte length — the same vectors the Rust gen-reference verify gate
// and the Python/Node SDKs check.
func TestFixtureVectorsHexExact(t *testing.T) {
	fixtures, _, _, _, _ := reference(t)
	for _, want := range fixtures {
		t.Run(want.Name, func(t *testing.T) {
			data, err := os.ReadFile(filepath.Join(repoRoot(t), "tests", "fixtures", want.Name+".pdf"))
			if err != nil {
				t.Fatal(err)
			}
			raw, err := ExtractCanonical(data)
			if err != nil {
				t.Fatalf("ExtractCanonical(%s): %v", want.Name, err)
			}
			canonical, err := ParseCanonical(raw)
			if err != nil {
				t.Fatal(err)
			}
			digest := sha256.Sum256(canonical.TextBytes)
			if got := hex.EncodeToString(digest[:]); got != want.TextSha {
				t.Errorf("%s: digest %s, want %s", want.Name, got, want.TextSha)
			}
			if len(canonical.TextBytes) != want.TextByte {
				t.Errorf("%s: text %d bytes, want %d", want.Name, len(canonical.TextBytes), want.TextByte)
			}
			if canonical.Pages != want.Pages {
				t.Errorf("%s: pages %d, want %d", want.Name, canonical.Pages, want.Pages)
			}
			if canonical.Rebuilt != want.Rebuilt {
				t.Errorf("%s: rebuilt %v, want %v", want.Name, canonical.Rebuilt, want.Rebuilt)
			}
		})
	}
}

// TestRecoveredStartxrefVector rebuilds the scan-recovery document from
// its inline hex and checks the rebuild is reported, never silent.
func TestRecoveredStartxrefVector(t *testing.T) {
	_, _, recovered, _, _ := reference(t)
	raw, err := ExtractCanonical(hexInput(t, recovered.Input))
	if err != nil {
		t.Fatal(err)
	}
	canonical, err := ParseCanonical(raw)
	if err != nil {
		t.Fatal(err)
	}
	digest := sha256.Sum256(canonical.TextBytes)
	if got := hex.EncodeToString(digest[:]); got != recovered.TextSha {
		t.Errorf("recovered_startxref: digest %s, want %s", got, recovered.TextSha)
	}
	if len(canonical.TextBytes) != recovered.TextByte {
		t.Errorf("recovered_startxref: text %d bytes, want %d", len(canonical.TextBytes), recovered.TextByte)
	}
	if canonical.Pages != recovered.Pages {
		t.Errorf("recovered_startxref: pages %d, want %d", canonical.Pages, recovered.Pages)
	}
	if !canonical.Rebuilt {
		t.Errorf("recovered_startxref: rebuilt = false, want true")
	}
}

// TestEncryptedFixtureRefusesUnsupported checks the /Encrypt refusal:
// status -3, never a crash.
func TestEncryptedFixtureRefusesUnsupported(t *testing.T) {
	_, encrypted, _, _, _ := reference(t)
	data, err := os.ReadFile(filepath.Join(repoRoot(t), "tests", "fixtures", "encrypted.pdf"))
	if err != nil {
		t.Fatal(err)
	}
	_, ferr := ExtractCanonical(data)
	ffi, ok := ferr.(*FfiError)
	if !ok {
		t.Fatalf("want FfiError, got %v", ferr)
	}
	if ffi.Status != StatusUnsupported {
		t.Errorf("encrypted: status %d, want %d (%s)", ffi.Status, StatusUnsupported, encrypted.Error)
	}
}

// TestOpenRefusalVectors replays every Document::open refusal row: the
// exact status (unsupported refusals are -3, everything else -2) and
// never a crash.
func TestOpenRefusalVectors(t *testing.T) {
	_, _, _, openRefusals, excludedCount := reference(t)
	if excludedCount != 11 || len(openRefusals) != 5 {
		t.Fatalf("refusal split changed: %d replayed + %d excluded, want 5 + 11 (decode_stream input echoes are filter names, not documents)",
			len(openRefusals), excludedCount)
	}
	for _, row := range openRefusals {
		t.Run(row.Name, func(t *testing.T) {
			_, err := ExtractCanonical(hexInput(t, row.Input))
			ffi, ok := err.(*FfiError)
			if !ok {
				t.Fatalf("want FfiError, got %v", err)
			}
			want := StatusRejected
			if strings.Contains(row.Error, "unsupported") {
				want = StatusUnsupported
			}
			if ffi.Status != want {
				t.Errorf("%s: status %d, want %d", row.Name, ffi.Status, want)
			}
		})
	}
}

// TestEmptyPagePinnedDigest pins one canonical-stream digest the Rust
// unit tests re-derive, so the binding fails loudly even if
// reference.json were regenerated wrongly.
func TestEmptyPagePinnedDigest(t *testing.T) {
	data, err := os.ReadFile(filepath.Join(repoRoot(t), "tests", "fixtures", "empty_page.pdf"))
	if err != nil {
		t.Fatal(err)
	}
	raw, err := ExtractCanonical(data)
	if err != nil {
		t.Fatal(err)
	}
	digest := sha256.Sum256(raw)
	const want = "5a8db37861d2ffa061f87ad2ec1b4c55181e695473ec20dbd228bad355b40e7e"
	if got := hex.EncodeToString(digest[:]); got != want {
		t.Errorf("empty_page: digest %s, want %s", got, want)
	}
	wantHeader := []byte{0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 14}
	for i, b := range wantHeader {
		if raw[i] != b {
			t.Fatalf("empty_page: header byte %d = %d, want %d", i, raw[i], b)
		}
	}
}

// TestMalformedInputIsRefused checks the extractor's refusal path: a
// status code, never a crash.
func TestMalformedInputIsRefused(t *testing.T) {
	_, err := ExtractCanonical([]byte("not a pdf at all"))
	var ffi *FfiError
	if e, ok := err.(*FfiError); ok {
		ffi = e
	} else {
		t.Fatalf("want FfiError, got %v", err)
	}
	if ffi.Status != StatusRejected {
		t.Errorf("want StatusRejected, got %d", ffi.Status)
	}
}

// hexInput decodes an inline-hex input echo from reference.json.
func hexInput(t *testing.T, s string) []byte {
	t.Helper()
	raw, err := hex.DecodeString(s)
	if err != nil {
		t.Fatal(err)
	}
	return raw
}
