#!/usr/bin/env bash
# Usage: scripts/mutants-summary.sh <mutants.out dir> <title>
# Prints a Markdown summary of a cargo-mutants run for $GITHUB_STEP_SUMMARY.
set -euo pipefail
out="${1:-mutants.out}"
title="${2:-Mutation testing}"
count() { if [ -f "$out/$1.txt" ]; then grep -c . "$out/$1.txt" || true; else echo 0; fi; }
caught=$(count caught)
missed=$(count missed)
timeout=$(count timeout)
unviable=$(count unviable)
echo "### $title"
echo
echo "| Caught | Missed | Timeout | Unviable |"
echo "| ---: | ---: | ---: | ---: |"
echo "| $caught | $missed | $timeout | $unviable |"
echo
if [ "$missed" -gt 0 ]; then
  echo "<details><summary>Missed mutants ($missed): no test failed when this code changed</summary>"
  echo
  echo '```'
  sort "$out/missed.txt"
  echo '```'
  echo "</details>"
  echo
fi
if [ "$timeout" -gt 0 ]; then
  echo "<details><summary>Timeouts ($timeout)</summary>"
  echo
  echo '```'
  sort "$out/timeout.txt"
  echo '```'
  echo "</details>"
fi
