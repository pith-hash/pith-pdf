// SPDX-License-Identifier: MIT
// Copyright (c) 2026 pith-hash

//go:build !windows && !cgo

package pithpdf

import "fmt"

// ffiExtractCanonical is unavailable without cgo on unix: there is no
// pure-Go dlopen in the standard library. Build with CGO_ENABLED=1
// (the CD pipeline always does).
func ffiExtractCanonical(string, *byte, int, **byte, *uintptr) (int32, error) {
	return 0, fmt.Errorf("pithpdf: cgo is required to load the cdylib on this platform (build with CGO_ENABLED=1)")
}

// ffiFree mirrors the unavailable extract.
func ffiFree(string, *byte, uintptr) {}
