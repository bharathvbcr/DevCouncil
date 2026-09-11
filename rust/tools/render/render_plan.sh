#!/usr/bin/env bash
#
# Render PLAN.md -> PLAN.html.
#
# Why this exists (DOC-1). PLAN.md's header advertised PLAN.html as the
# "rendered version, same content, styled". That was false: PLAN.html was a
# hand-maintained snapshot last written 2026-08-12, organised into 8 sections
# against PLAN.md's 11, and it drifted further with every edit to the markdown.
# A styled companion nobody can regenerate is a document that silently ages
# into a second, contradictory source of truth — and this repository already
# has a rule against that.
#
# The styling is the whole point of the file, so it is preserved verbatim
# rather than replaced with pandoc's default: `plan_head.html` holds the
# <title> + <style> block lifted from the 2026-08-12 revision. Change the CSS
# there, not here.
#
# Requires pandoc (`brew install pandoc`). It is a documentation tool, not a
# build or runtime dependency of the workspace — nothing in `cargo build`,
# `cargo test` or `verify.sh` calls this script, and the repository builds
# without it. It fails loudly rather than emitting a half-rendered file.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"

src="$root/PLAN.md"
head_tpl="$here/plan_head.html"
out="$root/PLAN.html"

if ! command -v pandoc >/dev/null 2>&1; then
  echo "render_plan.sh: pandoc not found; install it (brew install pandoc) or render PLAN.html another way." >&2
  echo "render_plan.sh: refusing to write a partial $out." >&2
  exit 1
fi

for f in "$src" "$head_tpl"; do
  [ -r "$f" ] || { echo "render_plan.sh: missing required input $f" >&2; exit 1; }
done

# Write through a temporary file so an interrupted run cannot leave PLAN.html
# truncated — a half-written render is the failure this script exists to stop.
tmp="$(mktemp "${TMPDIR:-/tmp}/plan_html.XXXXXX")"
trap 'rm -f "$tmp"' EXIT

cat "$head_tpl" > "$tmp"
printf '\n<div class="wrap">\n' >> "$tmp"

# gfm: PLAN.md uses GitHub tables throughout, which pandoc's strict markdown
# reader does not accept. --syntax-highlighting=none keeps the CSS in plan_head.html
# authoritative instead of injecting a second, competing colour scheme.
pandoc "$src" \
  --from=gfm \
  --to=html5 \
  --syntax-highlighting=none \
  --wrap=none \
  >> "$tmp"

printf '\n</div>\n' >> "$tmp"

mv "$tmp" "$out"
trap - EXIT

printf 'render_plan.sh: wrote %s (%s bytes) from %s\n' \
  "$out" "$(wc -c < "$out" | tr -d ' ')" "$src"
