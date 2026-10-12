#!/usr/bin/env bash
# Regenerates expected/ from the ORIGINAL transcribe.sh, run for real on five tone files
# with a stub whisper-cli (stub-whisper in this folder). Run once, when the Rust runner
# was ported; kept so the provenance of the goldens is not a mystery.
# Needs ffmpeg, jq and python3.
#
#   crates/ttrpg-core/tests/fixtures/run/generate.sh
#
# Wall-clock fields (timestamps, durations, sizes) and this machine's paths are
# normalised away, the same way the test normalises the Rust run's output.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../../../../.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

mkdir -p "$work"/{vault/0.2_campaigns/demo,audio/demo/"Session 7",home}
cp "$here/canon_aliases.json" "$work/vault/0.2_campaigns/demo/"
printf x > "$work/model.bin"
for n in 1-gmperson 2-riverone 3-rivertwo 4-plain_0 5-newperson; do
    ffmpeg -loglevel error -y -f lavfi -i "sine=frequency=440:duration=1" -ar 16000 -ac 1 \
        "$work/audio/demo/Session 7/$n.flac"
done

env -i PATH="$PATH" HOME="$work/home" LC_ALL=C \
    GM_VAULT="$work/vault" GM_AUDIO_ROOT="$work/audio" GM_WORK_ROOT="$work/work" \
    GM_CHROMA_ROOT="$work/chroma" GM_TRANSCRIBER_CONFIG="$work/none.json" \
    bash "$repo/crates/ttrpg-core/tests/oracle/transcribe.sh" --campaign demo --session 7 \
        -w "$here/stub-whisper" -m "$work/model.bin" \
        >"$work/stdout.txt"

session="$work/vault/0.2_campaigns/demo/08_transcripts/S07"
out="$here/expected"
rm -rf "$out"; mkdir -p "$out/processed"
cp "$session"/{merged_transcript.tsv,consolidated_transcript.tsv,transcript.md} "$out/"
cp "$work"/work/demo/S07/temp/*.processed.tsv "$out/processed/"
normalise() { sed -E -e "s#$work#<root>#g" -e 's/\([0-9]+ MB\)/(N MB)/' -e 's/ in [0-9]+s$/ in Ns/'; }
# Log: drop the timestamp column.
sed -E 's/^[0-9-]+ [0-9:]+,(INFO|WARNING|ERROR),/\1,/' "$session/transcription_log.csv" | normalise > "$out/log.txt"
# State: keep the file, status, player and error; drop size, seconds and time.
python3 - "$session/transcription_state.csv" > "$out/state.txt" <<'PY'
import csv, sys
for row in list(csv.reader(open(sys.argv[1], newline='')))[1:]:
    print("|".join([row[0], row[3], row[5], row[6]]))
PY
echo "wrote $out"
