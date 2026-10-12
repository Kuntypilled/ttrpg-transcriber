#!/usr/bin/env bash
# Regenerates the expected/ folders from the ORIGINAL script, in --post-process-only
# mode (no audio, no whisper). Run once, at the time the Rust pipeline was ported;
# kept so the provenance of the goldens is not a mystery. Needs jq and python3.
#
#   crates/ttrpg-core/tests/fixtures/pipeline/generate.sh
#
# C locale on purpose: it makes the script's sort and case folding the plain ones.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../../../../.." && pwd)"
script="$repo/legacy/transcribe.sh"

run_case() {
    local name="$1"; shift
    local out="$here/$name"
    rm -rf "$out"; mkdir -p "$out/work/archive"
    cp "$here"/archive/*.json "$out/work/archive/"
    ( cd "$out" && env -i PATH="$PATH" HOME="$out/home" LC_ALL=C \
        bash "$script" -p -o "$out/work" --temp-dir "$out/work/temp" --aliases "$here/aliases.json" "$@" >/dev/null )
    mkdir -p "$out/expected/processed"
    cp "$out"/work/{merged_transcript.tsv,consolidated_transcript.tsv,transcript.md} "$out/expected/"
    cp "$out"/work/temp/*.processed.tsv "$out/expected/processed/"
    # The script's own log lines for the stages, without their timestamps.
    grep -E "Processed:|Created |Creating |Aggregating|Consolidating" "$out/work/transcription_log.csv" \
        | sed -E 's/^[0-9-]+ [0-9:]+,INFO,//' | LC_ALL=C sort > "$out/expected/log_lines.txt"
    rm -rf "$out/work" "$out/home"
}

run_case default
run_case custom --ignore-words "hmm,okay" --stock-phrases "okay,thank you" --stock-min-seconds 5
run_case no_stock --stock-phrases ""
