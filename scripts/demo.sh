#!/usr/bin/env bash
# Try the TUI without whisper, a GPU or real recordings.
#
# Builds a throwaway world, then opens the TUI on it:
#   - an invented campaign (and a second one) in a vaults folder
#   - a Downloads folder holding six tiny "Craig" tracks, one from a speaker the
#     campaign does not know, so you can see the warning
#   - a stand-in whisper-cli that waits a few seconds per track and writes canned dialogue
#   - a stand-in for the Python that has ChromaDB, so Index and the alias report can be tried
#
# Nothing outside the demo folder is touched and your real config is ignored.
#
#   scripts/demo.sh              fresh demo world, then the TUI
#   scripts/demo.sh --keep       reuse the world from last time (to see skipped tracks, retries)
#   DEMO_DELAY=0 scripts/demo.sh   no pretend work, runs finish at once
#   DEMO_FAIL=1 scripts/demo.sh    add a seventh track whose transcription fails
#   DEMO_DIR=/some/path scripts/demo.sh
#   TTRPG_TUI_BIN=/path/to/ttrpg-tui scripts/demo.sh   use an existing build

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
demo="${DEMO_DIR:-${TMPDIR:-/tmp}/ttrpg-tui-demo}"
keep=false
[[ "${1:-}" == "--keep" ]] && keep=true

missing=()
for tool in bash python3 ffmpeg jq; do
    command -v "$tool" >/dev/null 2>&1 || missing+=("$tool")
done
if ((${#missing[@]})); then
    echo "The demo needs: ${missing[*]} (the real pipeline needs them too)." >&2
    exit 1
fi

# --- the demo world ------------------------------------------------------------------------
if $keep && [[ -f "$demo/.ttrpg-tui-demo" ]]; then
    echo "Reusing $demo"
else
    if [[ -e "$demo" && ! -f "$demo/.ttrpg-tui-demo" ]]; then
        echo "Refusing to replace $demo: it exists and was not made by this script." >&2
        echo "Pick another folder with DEMO_DIR=..." >&2
        exit 1
    fi
    rm -rf "$demo"
    mkdir -p "$demo/home/Downloads" "$demo/bin" "$demo/state"
    touch "$demo/.ttrpg-tui-demo"

    cp "$here/scripts/demo/fake-whisper-cli" "$demo/bin/whisper-cli"
    cp "$here/scripts/demo/fake-python" "$demo/bin/python"
    chmod +x "$demo/bin/whisper-cli" "$demo/bin/python"
    echo "not a real model" >"$demo/model.bin"

    camps="$demo/vaults/emberfall/0.2_campaigns"
    mkdir -p "$camps/main" "$demo/vaults/thornwake/0.2_campaigns/lantern"
    # Character names are stored with underscores, as the real files have them.
    sed -E '/"role": "Player"/ s/"character": "([A-Za-z]+) ([A-Za-z]+)"/"character": "\1_\2"/' \
        "$here/examples/canon_aliases.example.json" >"$camps/main/canon_aliases.json"
    cp "$here/crates/ttrpg-core/tests/fixtures/players_only.json" \
        "$demo/vaults/thornwake/0.2_campaigns/lantern/canon_aliases.json"

    # An earlier session that already has a transcript, so session 2 is offered next
    # and the Transcript tab has something to show straight away.
    s1="$camps/main/08_transcripts/S01"
    mkdir -p "$s1"
    cat >"$s1/transcript.md" <<'EOF'
**[00:00:04] GM:**
Three roads meet at the well in Emberfall. Only one of them is lit.

**[00:00:15] Vashti_Corrow:**
Then we take the dark one. Obviously.

**[00:00:21] Bram_Ollander:**
Obviously. I will bring a lantern anyway.

**[00:00:30] Nell:**
I will bring snacks. Someone has to think ahead.

EOF

    # Six tracks named the way Craig names them: N-discordusername.flac. The first five
    # match discord_names in the example campaign; the sixth does not.
    names=(gmuser someuser anotheruser thirduser fourthuser newguest)
    [[ "${DEMO_FAIL:-}" == 1 ]] && names+=(failing_mic)
    for i in "${!names[@]}"; do
        n=$((i + 1))
        ffmpeg -loglevel error -y -f lavfi -i "sine=frequency=$((200 + 55 * n)):duration=3" \
            -ar 16000 -ac 1 "$demo/home/Downloads/${n}-${names[$i]}.flac"
    done
fi

# --- the program --------------------------------------------------------------------------
bin="${TTRPG_TUI_BIN:-}"
if [[ -z "$bin" ]]; then
    command -v cargo >/dev/null 2>&1 || { echo "cargo is needed to build the TUI (or set TTRPG_TUI_BIN)." >&2; exit 1; }
    echo "Building the TUI (the first time takes a minute)..."
    (cd "$here" && cargo build --release -q -p ttrpg-tui)
    bin="$here/target/release/ttrpg-tui"
fi

# Everything the program reads comes from here, never from your real setup.
export HOME="$demo/home"
export XDG_STATE_HOME="$demo/state"
export GM_TRANSCRIBER_CONFIG="$demo/no-config.json"
export GM_VAULTS_ROOT="$demo/vaults"
export GM_AUDIO_ROOT="$demo/audio"
export GM_CHROMA_ROOT="$demo/chroma"
export GM_WORK_ROOT="$demo/work"
export GM_WHISPER_BIN="$demo/bin/whisper-cli"
export GM_WHISPER_MODEL="$demo/model.bin"
export GM_PYTHON="$demo/bin/python"
export GM_LEGACY_DIR="$here/legacy"
export DEMO_DELAY="${DEMO_DELAY:-2.5}"
unset GM_VAULT VAULT GM_WHISPER_DEVICE

cat <<EOF

Demo world: $demo
Try: f (choose files, they are in Downloads), a (all), c (done), s (start), Enter.
     Then look at the Log and Transcript tabs. ? lists every key.
     r resolves the unknown guest. With session 1 selected (n, 1, Enter), i indexes its
     transcript and a writes the alias report (the vector store is pretended; the rest is real).
     Lantern Row has no review_only list, so i asks about the report first there.

EOF
exec "$bin"
