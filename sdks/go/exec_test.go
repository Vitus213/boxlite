package boxlite

import (
	"context"
	"reflect"
	"strings"
	"testing"
)

// envMapToFlatPairs is the pure-Go pivot point of ExecutionOptions.Env
// support — it owns the contract that env_pairs lands in the C struct
// as a deterministic, even-length [k0,v0,k1,v1,...] sequence. The C
// side at sdks/c/src/exec/command.rs reads pairs in chunks of two
// and silently drops an odd trailing element, so any drift from the
// "sorted, flat, even-length" shape becomes a silent bug rather than
// a compile error. These cases pin the shape so a future refactor
// can't regress it unnoticed.
func TestEnvMapToFlatPairs(t *testing.T) {
	t.Run("nil yields nil", func(t *testing.T) {
		if got := envMapToFlatPairs(nil); got != nil {
			t.Fatalf("nil env must yield nil pairs, got %v", got)
		}
	})

	t.Run("empty yields nil", func(t *testing.T) {
		if got := envMapToFlatPairs(map[string]string{}); got != nil {
			t.Fatalf("empty env must yield nil pairs, got %v", got)
		}
	})

	t.Run("sorted by key, flat, even length", func(t *testing.T) {
		got := envMapToFlatPairs(map[string]string{
			"PATH":   "/usr/bin",
			"HOME":   "/root",
			"LANG":   "C",
			"DEBUG":  "1",
			"SHLVL":  "1",
			"PWD":    "/",
			"FOO":    "bar",
			"AAA":    "first",
			"ZZZ":    "last",
			"_UNDER": "score",
		})
		// 10 entries -> 20 strings.
		if len(got)%2 != 0 {
			t.Fatalf("flat pairs must have even length, got %d: %v", len(got), got)
		}
		want := []string{
			"AAA", "first",
			"DEBUG", "1",
			"FOO", "bar",
			"HOME", "/root",
			"LANG", "C",
			"PATH", "/usr/bin",
			"PWD", "/",
			"SHLVL", "1",
			"ZZZ", "last",
			"_UNDER", "score",
		}
		if !reflect.DeepEqual(got, want) {
			t.Fatalf("env pairs not in sorted key order\nwant: %v\n got: %v", want, got)
		}
	})

	t.Run("empty value preserved (env=) ", func(t *testing.T) {
		got := envMapToFlatPairs(map[string]string{"EMPTY": ""})
		want := []string{"EMPTY", ""}
		if !reflect.DeepEqual(got, want) {
			t.Fatalf("empty-value pair must round-trip, want %v got %v", want, got)
		}
	})
}

// The cgo boundary passes strings to C verbatim, but Rust reads them back with
// CStr::from_ptr, which stops at the first NUL. Unguarded, "\x001000:1000"
// reached the guest as "" — the box default, usually root — and
// "root\x00junk" as "root": a silent identity change the caller could not see.
func TestStartExecutionRejectsNULInUser(t *testing.T) {
	for _, user := range []string{"\x001000:1000", "root\x00invalid"} {
		// No runtime and no box handle: rejection must happen before the call
		// reaches the C boundary, so nothing is ever started.
		_, err := (&Box{}).StartExecution(context.Background(), "id", nil, &ExecutionOptions{User: user})
		if !IsInvalidArgument(err) {
			t.Fatalf("user %q: got %v, want ErrInvalidArgument", user, err)
		}
		if want := "user must not contain NUL bytes"; !strings.Contains(err.Error(), want) {
			t.Fatalf("user %q: error %q does not report %q", user, err, want)
		}
	}
}

// A NUL-free user must still cross the boundary untouched: it is the C side's
// nil-handle check that rejects this call, not the guard.
func TestStartExecutionPassesNULFreeUser(t *testing.T) {
	_, err := (&Box{}).StartExecution(context.Background(), "id", nil, &ExecutionOptions{User: "1000:1000"})
	if err == nil || strings.Contains(err.Error(), "NUL") {
		t.Fatalf("NUL-free user must reach the C boundary, got %v", err)
	}
}
