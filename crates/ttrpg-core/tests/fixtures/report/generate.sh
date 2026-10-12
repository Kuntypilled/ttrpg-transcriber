#!/usr/bin/env bash
# Regenerates expected_<case>.md from the ORIGINAL report code
# (tests/oracle/transcript_to_chunks.py --report), with the word list replaced by words.txt
# so the output does not depend on the machine's /usr/share/dict. Run once, when the
# Rust report was ported; kept for provenance. Needs python3.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../../../../.." && pwd)"
tmp="$(mktemp -d)"
camp="$tmp/vault/0.2_campaigns/demo"
mkdir -p "$camp/08_transcripts/S03"
cp "$here/canon_aliases.json" "$camp/"
cp "$here/transcript.md" "$camp/08_transcripts/S03/transcript.md"
env -i PATH="$PATH" HOME="$tmp/home" GM_VAULT="$tmp/vault" GM_TRANSCRIBER_CONFIG="$tmp/none.json" \
    GM_AUDIO_ROOT="$tmp/audio" GM_CHROMA_ROOT="$tmp/chroma" GM_WORK_ROOT="$tmp/work" \
    python3 -I -B - "$repo/crates/ttrpg-core/tests/oracle" "$here" "$tmp" <<'PY'
import json, sys
oracle, here, tmp = sys.argv[1:4]
sys.path.insert(0, oracle)
import transcript_to_chunks as t
for case in json.load(open(f"{here}/cases.json")):
    t.WORDLISTS = (f"{here}/words.txt",) if case["dictionary"] else ("/nonexistent/words",)
    out = f"{here}/expected_{case['name']}.md"
    code = t.main(["--campaign", "demo", "--session", "3", "--report", "-o", out,
                   "--threshold", str(case["threshold"]), "--min-freq", str(case["min_freq"])])
    assert code == 0
PY
# The report names the transcript it read: make that path the same every time.
sed -i "s#$tmp#/TMP#g" "$here"/expected_*.md
rm -rf "$tmp"
