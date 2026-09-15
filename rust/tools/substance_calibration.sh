#!/usr/bin/env bash
# Calibrate the substance gate's threshold against real history.
#
# `dc-verify/src/substance.rs` classifies every added line of a diff as
# substantive, trivial, moved, repeated or generated. The ratio of substantive
# to added lines is only useful if the threshold below which it is called "low"
# sits under the range ordinary work occupies — otherwise the gate fires on
# normal development and gets ignored, which is the failure mode every rigor
# gate in this crate is written to avoid.
#
# So the threshold is measured rather than chosen. This runs the real dcverify
# binary over one commit at a time and prints the distribution, plus every
# commit that would fall under the current constant, so each one can be
# inspected and the constant defended by name.
#
# Usage:
#   ./tools/substance_calibration.sh [commits] [repo]
#
# Defaults to the last 200 commits of the repository this file lives in.
set -euo pipefail
cd "$(dirname "$0")/.."

COMMITS="${1:-200}"
REPO="${2:-$(cd .. && pwd)}"

if ! [[ "$COMMITS" =~ ^[0-9]+$ ]] || [ "$COMMITS" -lt 1 ]; then
  echo "commits must be a positive integer, got: $COMMITS" >&2
  exit 2
fi

# The binary this repository builds, not one from PATH. An installed dcverify
# is a different program from the one in this tree, and a calibration that
# measured it would be describing code nobody is about to change.
DCVERIFY="${CARGO_TARGET_DIR:-$(pwd)/target}/debug/dcverify"
if [ ! -x "$DCVERIFY" ]; then
  echo "building dcverify…" >&2
  cargo build -p dc-verify --bin dcverify >&2
fi
[ -x "$DCVERIFY" ] || { echo "no dcverify at $DCVERIFY" >&2; exit 1; }

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
SAMPLES="$TMP/samples.tsv"
: > "$SAMPLES"

# --no-merges: a merge commit's diff against its first parent re-lists every
# line the branch touched, which would enter the distribution as one enormous
# sample that no single act of development produced.
#
# A `while read` loop rather than `mapfile`: macOS ships bash 3.2, where
# mapfile does not exist, and this repository's CI runs macos-15 as well as
# ubuntu. A calibration tool that only runs on one of the two platforms is a
# tool that stops being run.
git -C "$REPO" log --no-merges --format=%H -n "$COMMITS" > "$TMP/shas"
echo "measuring $(wc -l < "$TMP/shas" | tr -d ' ') commits…" >&2

while IFS= read -r sha; do
  [ -n "$sha" ] || continue
  # --format= suppresses the commit message; a message body is not part of the
  # change and its lines are not added lines.
  json=$(git -C "$REPO" show --format= --unified=3 "$sha" | "$DCVERIFY" check 2>/dev/null || true)
  [ -n "$json" ] || continue
  printf '%s\t%s\n' "$sha" "$json" >> "$SAMPLES"
done < "$TMP/shas"

perl -MJSON::PP -e '
  my (@ratios, @low, $judged, $total);
  my $num = $ARGV[1]; my $den = $ARGV[2];
  open my $fh, "<", $ARGV[0] or die $!;
  while (my $line = <$fh>) {
    chomp $line;
    my ($sha, $json) = split /\t/, $line, 2;
    next unless defined $json && length $json;
    my $r = eval { decode_json($json) } or next;
    next unless $r->{ok} && ref $r->{substance} eq "HASH";
    my $s = $r->{substance};
    $total++;
    next unless $s->{judged};
    $judged++;
    my $ratio = $s->{added_lines} ? $s->{substantive_lines} / $s->{added_lines} : 0;
    push @ratios, $ratio;
    push @low, sprintf("%.3f  %s  %d/%d added  (trivial %d, moved %d, repeated %d, generated %d)",
        $ratio, substr($sha, 0, 12), $s->{substantive_lines}, $s->{added_lines},
        $s->{trivial}, $s->{moved}, $s->{repeated}, $s->{generated})
      if $s->{substantive_lines} * $den < $s->{added_lines} * $num;
  }
  die "no commit produced a parsable measurement\n" unless $total;
  die "no commit was large enough to judge\n" unless $judged;
  @ratios = sort { $a <=> $b } @ratios;
  # Nearest-rank percentile: with ~180 samples an interpolating definition
  # invents precision the sample size does not carry.
  my $pct = sub { $ratios[ int( $_[0] / 100 * (@ratios - 1) + 0.5 ) ] };
  printf "commits measured: %d   judged (>= MIN_LINES_TO_JUDGE): %d\n\n", $total, $judged;
  printf "%-12s %s\n", "percentile", "substantive / added";
  printf "%-12s %.2f\n", "minimum", $ratios[0];
  printf "%-12s %.2f\n", "5th",    $pct->(5);
  printf "%-12s %.2f\n", "25th",   $pct->(25);
  printf "%-12s %.2f\n", "median", $pct->(50);
  printf "%-12s %.2f\n", "75th",   $pct->(75);
  printf "%-12s %.2f\n", "maximum", $ratios[-1];
  printf "\nbelow the current threshold (%d/%d): %d commit(s)\n", $num, $den, scalar @low;
  print "  $_\n" for @low;
' "$SAMPLES" \
  "$(grep -oE 'LOW_SUBSTANCE_NUMERATOR: usize = [0-9]+' dc-verify/src/substance.rs | grep -oE '[0-9]+$')" \
  "$(grep -oE 'LOW_SUBSTANCE_DENOMINATOR: usize = [0-9]+' dc-verify/src/substance.rs | grep -oE '[0-9]+$')"
