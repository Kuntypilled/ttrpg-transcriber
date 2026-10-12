#!/usr/bin/env bash
# Regenerates expected.jsonl and expected_summary.txt from the ORIGINAL chunker
# (tests/oracle/transcript_to_chunks.py). Run once, when the Rust chunker was ported; kept
# for provenance. Needs python3.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../../../../.." && pwd)"
for case in basic edges; do
    tmp="$(mktemp -d)"
    camp="$tmp/vault/0.2_campaigns/demo"
    mkdir -p "$camp/08_transcripts/S03"
    cp "$here/$case/canon_aliases.json" "$camp/"
    cp "$here/$case/transcript.md" "$camp/08_transcripts/S03/transcript.md"
    env -i PATH="$PATH" HOME="$tmp/home" GM_VAULT="$tmp/vault" GM_TRANSCRIBER_CONFIG="$tmp/none.json" \
        GM_AUDIO_ROOT="$tmp/audio" GM_CHROMA_ROOT="$tmp/chroma" GM_WORK_ROOT="$tmp/work" \
        python3 "$repo/crates/ttrpg-core/tests/oracle/transcript_to_chunks.py" --campaign demo --session 3 --date 2026-01-02 \
        -o "$here/$case/expected.jsonl" 2> "$tmp/stderr"
    head -n1 "$tmp/stderr" > "$here/$case/expected_summary.txt"
    rm -rf "$tmp"
done
