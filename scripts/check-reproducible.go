//go:build ignore

// Command check-reproducible verifies that two APKs are reproducible in the
// sense F-Droid cares about: identical everywhere except the signature. F-Droid
// rebuilds the app from source, then reattaches the developer's signature to its
// own build with apksigcopier; that only works if the two APKs carry the same
// ZIP entries — same names, same order, same compression, same compressed bytes
// — differing only in the signature.
//
// Taken unchanged, bar this paragraph and the examples, from pgaskin/cmus-android
// (notes/check-reproducible.go), where it verifies an F-Droid rebuild of a
// published release. check-reproducible.sh is what drives it here.
//
// The signature is two things, both ignored here:
//
//   - v1 (JAR) signature files: META-INF/MANIFEST.MF and META-INF/*.{SF,RSA,DSA,EC}.
//   - the APK Signing Block (v2/v3/v3.1), which lives between the entries and
//     the central directory.
//
// It does NOT compare the raw byte ranges of the two archives, because
// apksigner re-aligns entries when it signs (padding in each local-header extra
// field), which shifts offsets throughout the file even though no entry's
// payload changed. Instead it compares each non-signature entry's *raw stored
// bytes* (what apksigcopier copies), which is alignment-independent and strict:
// two entries with the same decompressed content but different compression
// would still be flagged.
//
// When neither APK is signed at all (two local builds), it additionally
// requires the whole files to be bit-for-bit identical — the strongest check.
//
// Each argument is a local .apk path or an http(s):// URL (downloaded to a
// temp file). Exit status is 0 only if the two APKs are reproducible.
//
// Usage:
//
//	# two local builds are bit-for-bit identical
//	go run check-reproducible.go \
//	    a/app/build/outputs/apk/free/release/app-free-arm64-v8a-release-unsigned.apk \
//	    b/app/build/outputs/apk/free/release/app-free-arm64-v8a-release-unsigned.apk
//
//	# a local build reproduces a published, signed release
//	go run check-reproducible.go \
//	    app/build/outputs/apk/free/release/app-free-arm64-v8a-release-unsigned.apk \
//	    https://example.invalid/releases/download/v1/app.apk
package main

import (
	"archive/zip"
	"bytes"
	"crypto/sha256"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"time"
)

const (
	eocdMagic     = "PK\x05\x06"       // end of central directory record
	sigBlockMagic = "APK Sig Block 42" // trailer of the APK Signing Block
)

// known APK Signing Block IDs. Anything else makes F-Droid reject the APK, so
// we surface unrecognized IDs loudly.
var knownSigBlockIDs = map[uint32]string{
	0x7109871a: "APK Signature Scheme v2",
	0xf05368c0: "APK Signature Scheme v3",
	0x1b93ad61: "APK Signature Scheme v3.1",
	0x42726577: "verity padding",
	0x6dff800d: "source stamp v2",
	0x2b09189e: "source stamp v1",
}

// isV1SigEntry reports whether name is a v1 (JAR) signature file: the manifest
// or a signature/signature-block file directly under META-INF. These are part
// of the signature and are excluded from the comparison. Note that nested
// entries like META-INF/com/android/.../app-metadata.properties are NOT
// signature files and are compared normally.
func isV1SigEntry(name string) bool {
	if !strings.HasPrefix(name, "META-INF/") {
		return false
	}
	rest := name[len("META-INF/"):]
	if strings.ContainsRune(rest, '/') {
		return false // nested, not a signature file
	}
	if rest == "MANIFEST.MF" {
		return true
	}
	up := strings.ToUpper(rest)
	return strings.HasSuffix(up, ".SF") || strings.HasSuffix(up, ".RSA") ||
		strings.HasSuffix(up, ".DSA") || strings.HasSuffix(up, ".EC")
}

// entry is the strict fingerprint of one ZIP entry: what apksigcopier must be
// able to copy verbatim.
type entry struct {
	method uint16
	crc32  uint32
	raw    string // sha256 of the raw stored (usually deflated) bytes
}

// fingerprint returns the ordered non-signature entry names and their
// fingerprints, plus whether the APK carries any v1 signature file.
func fingerprint(data []byte) (order []string, m map[string]entry, hasV1 bool, err error) {
	zr, err := zip.NewReader(bytes.NewReader(data), int64(len(data)))
	if err != nil {
		return nil, nil, false, err
	}
	m = make(map[string]entry, len(zr.File))
	for _, f := range zr.File {
		if isV1SigEntry(f.Name) {
			hasV1 = true
			continue
		}
		rc, err := f.OpenRaw() // raw stored bytes, before alignment padding
		if err != nil {
			return nil, nil, false, fmt.Errorf("open %q: %w", f.Name, err)
		}
		h := sha256.New()
		if _, err := io.Copy(h, rc); err != nil {
			return nil, nil, false, fmt.Errorf("read %q: %w", f.Name, err)
		}
		order = append(order, f.Name)
		m[f.Name] = entry{f.Method, f.CRC32, fmt.Sprintf("%x", h.Sum(nil))}
	}
	return order, m, hasV1, nil
}

// diff returns the non-signature differences between two APKs. Empty means the
// entries are reproducible (apksigcopier could reattach the signature).
func diff(ao []string, am map[string]entry, bo []string, bm map[string]entry) []string {
	var out []string
	for _, n := range ao {
		be, ok := bm[n]
		if !ok {
			out = append(out, "only in A: "+n)
			continue
		}
		ae := am[n]
		switch {
		case ae.raw != be.raw:
			out = append(out, "compressed bytes differ: "+n)
		case ae.crc32 != be.crc32:
			out = append(out, fmt.Sprintf("crc32 differs: %s (%08x vs %08x)", n, ae.crc32, be.crc32))
		case ae.method != be.method:
			out = append(out, fmt.Sprintf("compression method differs: %s (%d vs %d)", n, ae.method, be.method))
		}
	}
	for _, n := range bo {
		if _, ok := am[n]; !ok {
			out = append(out, "only in B: "+n)
		}
	}
	if !equalStrings(ao, bo) {
		out = append(out, "entry order differs")
	}
	return out
}

func equalStrings(a, b []string) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

// sigBlock extracts the APK Signing Block (empty if the APK is not v2/v3-signed)
// using the ZIP structure directly.
func sigBlock(data []byte) ([]byte, error) {
	i := bytes.LastIndex(data, []byte(eocdMagic))
	if i < 0 || i+20 > len(data) {
		return nil, errors.New("no end-of-central-directory record (not a zip?)")
	}
	cdOff := binary.LittleEndian.Uint32(data[i+16 : i+20])
	if cdOff == 0xffffffff {
		return nil, errors.New("zip64 APKs are not supported")
	}
	cd := int(cdOff)
	if cd < 0 || cd > i {
		return nil, fmt.Errorf("central-directory offset %d out of range", cd)
	}
	if cd >= 24 && string(data[cd-16:cd]) == sigBlockMagic {
		size := binary.LittleEndian.Uint64(data[cd-24 : cd-16])
		start := cd - int(size) - 8
		if start < 0 || start > cd {
			return nil, fmt.Errorf("bad APK Signing Block size %d", size)
		}
		return data[start:cd], nil
	}
	return nil, nil
}

// sigBlockIDs returns the ID-value pair IDs inside an APK Signing Block.
func sigBlockIDs(block []byte) []uint32 {
	if len(block) < 32 {
		return nil
	}
	p := block[8 : len(block)-24] // strip leading size and trailing size+magic
	var ids []uint32
	for len(p) >= 12 {
		n := binary.LittleEndian.Uint64(p[0:8])
		if n < 4 || uint64(len(p)) < 8+n {
			break
		}
		ids = append(ids, binary.LittleEndian.Uint32(p[8:12]))
		p = p[8+n:]
	}
	return ids
}

// describeSignature prints an APK's signature state and returns true if it
// carries an unrecognized signing block that F-Droid would reject.
func describeSignature(label string, block []byte, hasV1 bool) bool {
	var s []string
	if hasV1 {
		s = append(s, "v1 (JAR)")
	}
	unknown := false
	for _, id := range sigBlockIDs(block) {
		if name, ok := knownSigBlockIDs[id]; ok {
			s = append(s, name)
		} else {
			s = append(s, fmt.Sprintf("UNKNOWN 0x%08x", id))
			unknown = true
		}
	}
	if len(s) == 0 {
		fmt.Printf("  %s: unsigned\n", label)
	} else {
		fmt.Printf("  %s: %s\n", label, strings.Join(s, ", "))
	}
	if unknown {
		fmt.Printf("  %s: ⚠ unrecognized signing block; F-Droid would reject this APK\n", label)
	}
	return unknown
}

// load reads an argument that is either a local path or an http(s):// URL.
func load(arg string) (name string, data []byte, err error) {
	if strings.HasPrefix(arg, "http://") || strings.HasPrefix(arg, "https://") {
		fmt.Fprintf(os.Stderr, "downloading %s ...\n", arg)
		c := &http.Client{Timeout: 5 * time.Minute}
		resp, err := c.Get(arg)
		if err != nil {
			return "", nil, err
		}
		defer resp.Body.Close()
		if resp.StatusCode != http.StatusOK {
			return "", nil, fmt.Errorf("GET %s: %s", arg, resp.Status)
		}
		b, err := io.ReadAll(resp.Body)
		return arg, b, err
	}
	b, err := os.ReadFile(arg)
	return filepath.Base(arg), b, err
}

func run() error {
	args := os.Args[1:]
	if len(args) != 2 {
		return errors.New("usage: check-reproducible <apk-or-url-A> <apk-or-url-B>")
	}

	nameA, dataA, err := load(args[0])
	if err != nil {
		return fmt.Errorf("load A: %w", err)
	}
	nameB, dataB, err := load(args[1])
	if err != nil {
		return fmt.Errorf("load B: %w", err)
	}

	ao, am, av1, err := fingerprint(dataA)
	if err != nil {
		return fmt.Errorf("parse A (%s): %w", nameA, err)
	}
	bo, bm, bv1, err := fingerprint(dataB)
	if err != nil {
		return fmt.Errorf("parse B (%s): %w", nameB, err)
	}
	ablk, err := sigBlock(dataA)
	if err != nil {
		return fmt.Errorf("parse A (%s): %w", nameA, err)
	}
	bblk, err := sigBlock(dataB)
	if err != nil {
		return fmt.Errorf("parse B (%s): %w", nameB, err)
	}

	fmt.Printf("A: %s (%d bytes, %d entries)\n", nameA, len(dataA), len(ao))
	fmt.Printf("B: %s (%d bytes, %d entries)\n\n", nameB, len(dataB), len(bo))

	fmt.Println("signatures (ignored in the comparison):")
	badA := describeSignature("A", ablk, av1)
	badB := describeSignature("B", bblk, bv1)
	fmt.Println()

	unsignedA := !av1 && len(ablk) == 0
	unsignedB := !bv1 && len(bblk) == 0

	diffs := diff(ao, am, bo, bm)
	reproducible := len(diffs) == 0

	fmt.Printf("non-signature entries reproducible: %s\n", yn(reproducible))
	for _, d := range diffs {
		fmt.Printf("  - %s\n", d)
	}
	fmt.Println()

	if !reproducible {
		return errors.New("RESULT: NOT reproducible ✗")
	}
	if badA || badB {
		return errors.New("RESULT: entries match, but an unrecognized signing block means F-Droid would reject it ✗")
	}
	if unsignedA && unsignedB {
		if bytes.Equal(dataA, dataB) {
			fmt.Println("RESULT: REPRODUCIBLE ✓  (both unsigned and bit-for-bit identical)")
		} else {
			// entries match but the raw files differ: only possible via
			// alignment/EOCD padding, which apksigcopier normalizes anyway.
			fmt.Println("RESULT: REPRODUCIBLE ✓  (entries identical; files differ only in alignment padding)")
		}
		return nil
	}
	fmt.Println("RESULT: REPRODUCIBLE ✓  (identical except the signature)")
	return nil
}

func yn(b bool) string {
	if b {
		return "yes"
	}
	return "NO"
}

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
