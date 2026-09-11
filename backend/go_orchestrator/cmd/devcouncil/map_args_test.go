package main

import (
	"reflect"
	"testing"
)

func TestMapArgs(t *testing.T) {
	t.Parallel()
	cases := []struct {
		name string
		in   []string
		want []string
	}{
		{name: "bare", in: nil, want: []string{"build", "--manifest"}},
		{name: "empty slice", in: []string{}, want: []string{"build", "--manifest"}},
		{name: "status", in: []string{"status"}, want: []string{"status"}},
		{name: "status --json", in: []string{"status", "--json"}, want: []string{"status", "--json"}},
		// --json is a kernel global. Bare `dev map --json` must still name a
		// subcommand or clap refuses; the default stays build --manifest.
		{name: "--json only", in: []string{"--json"}, want: []string{"--json", "build", "--manifest"}},
		{name: "--json paths", in: []string{"--json", "paths"}, want: []string{"--json", "paths"}},
		{name: "--json status", in: []string{"--json", "status"}, want: []string{"--json", "status"}},
		{name: "--root DIR", in: []string{"--root", "/tmp"}, want: []string{"--root", "/tmp", "build", "--manifest"}},
		{name: "--root DIR status", in: []string{"--root", "/tmp", "status"}, want: []string{"--root", "/tmp", "status"}},
		{name: "--db=file status", in: []string{"--db=store.sqlite", "status"}, want: []string{"--db=store.sqlite", "status"}},
		{name: "--full (build flag)", in: []string{"--full"}, want: []string{"build", "--manifest", "--full"}},
		{name: "--json --full", in: []string{"--json", "--full"}, want: []string{"--json", "build", "--manifest", "--full"}},
		{name: "--help stays help", in: []string{"--help"}, want: []string{"--help"}},
		{name: "-h stays help", in: []string{"-h"}, want: []string{"-h"}},
		{name: "--version stays version", in: []string{"--version"}, want: []string{"--version"}},
		{name: "--json --help", in: []string{"--json", "--help"}, want: []string{"--json", "--help"}},
		{name: "--root missing value", in: []string{"--root"}, want: []string{"--root"}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()
			got := mapArgs(tc.in)
			if !reflect.DeepEqual(got, tc.want) {
				t.Fatalf("mapArgs(%q) = %v, want %v", tc.in, got, tc.want)
			}
		})
	}
}

func TestAstArgsPrependsAst(t *testing.T) {
	t.Parallel()
	got := astArgs([]string{"--json", "Foo"})
	want := []string{"ast", "--json", "Foo"}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("astArgs = %v, want %v", got, want)
	}
	if got := astArgs(nil); !reflect.DeepEqual(got, []string{"ast"}) {
		t.Fatalf("astArgs(nil) = %v, want [ast]", got)
	}
}
