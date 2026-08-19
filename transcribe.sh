#!/usr/bin/env bash
# transcribe.sh: TTRPG session transcription pipeline
#
# Linux, any distribution. Written against an AMD GPU with the whisper.cpp
# Vulkan backend, but nothing here is vendor-specific: whisper-cli just has to
# be on PATH, built with whatever backend your hardware wants.
#
# Uses whisper.cpp with ggml-large-v3-turbo, on Craig Discord bot multitrack
# recordings.
#
# Pipeline:
#   1. Transcribe each audio track via whisper.cpp
#   2. Add speaker column from filename (discord_names lookup → N-playername_M.ext fallback)
#   3. Filter filler words, collapse duplicate consecutive lines
#   4. Merge all tracks sorted by timestamp → merged_transcript.tsv
#   5. Consolidate consecutive same-speaker lines → consolidated_transcript.tsv
#   6. Generate human-readable → transcript.md
#
# Requirements:
#   - whisper-cli  (build: cmake -B build -DGGML_VULKAN=ON && cmake --build build -j$(nproc))
#   - ffmpeg       (your distribution's package manager)
#   - jq           (your distribution's package manager)
#   - Model:       ggml-large-v3-turbo.bin from huggingface.co/ggerganov/whisper.cpp
#
# Run  ./transcribe.sh --check  and it will tell you what is missing and give
# you the install command for the package manager it finds on this machine.
#
# Usage:
#   ./transcribe.sh --campaign my_campaign --session 17
#   ./transcribe.sh -i /path/to/recordings --aliases /path/to/canon_aliases.json
#   ./transcribe.sh --campaign my_campaign --session 17 --post-process-only
#   ./transcribe.sh --campaign my_campaign --session 17 --force --cleanup
#
# --campaign resolves every path from the vault by asking campaign.py, so the
# path rules live in exactly one place and are not duplicated here:
#   input   $GM_AUDIO_ROOT/<campaign>/Session <n>
#   output  $GM_VAULT/<campaigns>/<campaign>/<transcripts>/S<nn>/
#   temp    $GM_WORK_ROOT/<campaign>/S<nn>/temp   (outside the vault; see below)
#   aliases $GM_VAULT/<campaigns>/<campaign>/canon_aliases.json
#
# The whisper working files (.raw.tsv, .processed.tsv, per-track .json) are
# intermediates that Obsidian ignores but Obsidian Sync does not, so temp/ lives
# outside the vault by default. archive/ stays in the vault: it holds the raw
# whisper JSON, which is what --post-process-only replays and the only artifact
# you cannot rebuild once the audio is gone.
#
# Audio file naming convention (Craig bot default):
#   N-playername_M.ext  (e.g. 2-playerone_0.flac, 6-gmhandle__0.mp3)
# Or with discord_names lookup: raw Discord usernames auto-resolved via the
# selected campaign's canon_aliases.json

set -euo pipefail

# ── Defaults ──────────────────────────────────────────────────────────────────

DEFAULT_MODEL="${HOME}/.local/share/whisper.cpp/models/ggml-large-v3-turbo.bin"
WHISPER_BIN="whisper-cli"

SCRIPT_DIR="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")" && pwd)"
CAMPAIGN_PY="$SCRIPT_DIR/campaign.py"

# Print the install command for whichever package manager this machine has.
# Package names are only mapped where they actually differ between families;
# ffmpeg and jq are spelled the same everywhere, which is why they take one arg.
install_hint() {
    local generic="$1" arch="${2:-$1}" deb="${3:-$1}" rpm="${4:-$1}"
    if   command -v pacman  &>/dev/null; then echo "sudo pacman -S --needed $arch"
    elif command -v apt     &>/dev/null; then echo "sudo apt install $deb"
    elif command -v dnf     &>/dev/null; then echo "sudo dnf install $rpm"
    elif command -v zypper  &>/dev/null; then echo "sudo zypper install $rpm"
    else echo "install '$generic' with your package manager"
    fi
}

# canon_aliases.json feeds (1) the Whisper seed prompt and (2) discord_names lookup.
# Which one is decided by --campaign (or --aliases); there is no global default.
ALIASES_FILE=""
WHISPER_PROMPT=""

# Filler words/short phrases to drop (exact match or with trailing punctuation)
DEFAULT_IGNORE_WORDS=("you" "silence" "um" "uh" "ah" "like" "right" "well")

# ── Usage ──────────────────────────────────────────────────────────────────────

usage() {
    cat <<'EOF'
Usage: transcribe.sh --campaign SLUG --session N [OPTIONS]
       transcribe.sh -i INPUT_FOLDER [OPTIONS]

Campaign mode (resolves every path from the vault):
      --campaign SLUG         Campaign folder under $VAULT/0.2_campaigns/
      --session N             Session number

Manual mode:
  -i, --input-folder DIR      Folder with audio files from Craig bot

Options:
  -o, --output-folder DIR     Output destination (default: ../transcriptions)
      --aliases PATH          canon_aliases.json to use (default: the campaign's)
      --temp-dir DIR          Whisper working files (default: outside the vault
                              in campaign mode, OUTPUT_FOLDER/temp otherwise)
  -m, --model PATH            Path to ggml model bin (default: ~/.local/share/whisper.cpp/models/ggml-large-v3-turbo.bin)
  -w, --whisper-bin NAME      whisper.cpp binary name or path (default: whisper-cli)
  -f, --force                 Re-transcribe already-processed files
  -p, --post-process-only     Skip transcription; re-run post-processing from archive
  -c, --cleanup               Remove temp/ directory after processing
      --ignore-words WORDS    Comma-separated filter words (replaces defaults)
      --check                 Verify dependencies and model, then exit
  -h, --help                  Show this help

Output files (in OUTPUT_FOLDER):
  merged_transcript.tsv       All speakers merged, sorted by timestamp
  consolidated_transcript.tsv Consecutive same-speaker lines joined
  transcript.md               Human-readable formatted transcript
  transcription_log.csv       Run log
  transcription_state.csv     Per-file status tracking
  archive/                    Original whisper.cpp JSON output

Working files (in TEMP_DIR, outside the vault in campaign mode):
  temp/                       Processed per-track TSVs (removed with --cleanup)

EOF
    exit 0
}

# ── Argument Parsing ───────────────────────────────────────────────────────────

INPUT_FOLDER=""
OUTPUT_FOLDER=""
CAMPAIGN=""
SESSION=""
ALIASES_OVERRIDE=""
TEMP_DIR_OVERRIDE=""
MODEL_PATH="$DEFAULT_MODEL"
FORCE=false
POST_PROCESS_ONLY=false
CLEANUP=false
CHECK_ONLY=false
IGNORE_WORDS=("${DEFAULT_IGNORE_WORDS[@]}")

while [[ $# -gt 0 ]]; do
    case "$1" in
        -i|--input-folder)      INPUT_FOLDER="$2";    shift 2 ;;
        -o|--output-folder)     OUTPUT_FOLDER="$2";   shift 2 ;;
        --campaign)             CAMPAIGN="$2";        shift 2 ;;
        --session)              SESSION="$2";         shift 2 ;;
        --aliases)              ALIASES_OVERRIDE="$2"; shift 2 ;;
        --temp-dir)             TEMP_DIR_OVERRIDE="$2"; shift 2 ;;
        -m|--model)             MODEL_PATH="$2";      shift 2 ;;
        -w|--whisper-bin)       WHISPER_BIN="$2";     shift 2 ;;
        -f|--force)             FORCE=true;           shift   ;;
        -p|--post-process-only) POST_PROCESS_ONLY=true; shift ;;
        -c|--cleanup)           CLEANUP=true;         shift   ;;
        --ignore-words)         IFS=',' read -ra IGNORE_WORDS <<< "$2"; shift 2 ;;
        --check)                CHECK_ONLY=true;      shift   ;;
        -h|--help)              usage ;;
        *) echo "Unknown option: $1" >&2; usage ;;
    esac
done

# ── Campaign Resolution ────────────────────────────────────────────────────────
# campaign.py owns the path rules. Asking it keeps them out of this script.

if [[ -n "$CAMPAIGN" ]]; then
    paths_args=(paths --campaign "$CAMPAIGN")
    [[ -n "$SESSION" ]] && paths_args+=(--session "$SESSION")
    if ! campaign_env="$(python3 "$CAMPAIGN_PY" "${paths_args[@]}")"; then
        echo "ERROR: could not resolve campaign '$CAMPAIGN'." >&2
        exit 1
    fi
    eval "$campaign_env"
    [[ -z "$INPUT_FOLDER"      && -n "${AUDIO_DIR:-}"   ]] && INPUT_FOLDER="$AUDIO_DIR"
    [[ -z "$OUTPUT_FOLDER"     && -n "${SESSION_DIR:-}" ]] && OUTPUT_FOLDER="$SESSION_DIR"
    [[ -z "$TEMP_DIR_OVERRIDE" && -n "${WORK_DIR:-}"    ]] && TEMP_DIR_OVERRIDE="$WORK_DIR/temp"
fi

# An explicit --aliases always wins over the campaign's own file.
[[ -n "$ALIASES_OVERRIDE" ]] && ALIASES_FILE="$ALIASES_OVERRIDE"
if [[ -n "$ALIASES_FILE" ]]; then
    WHISPER_PROMPT="$(jq -r '.seed_prompt // empty' "$ALIASES_FILE" 2>/dev/null || true)"
fi

# --post-process-only replays archived whisper JSON, so it needs no audio at all.
if [[ -z "$INPUT_FOLDER" ]] && ! $CHECK_ONLY && ! $POST_PROCESS_ONLY; then
    echo "ERROR: give --campaign SLUG --session N, or --input-folder DIR." >&2
    usage
fi

if [[ -n "$INPUT_FOLDER" && ! -d "$INPUT_FOLDER" ]] && ! $POST_PROCESS_ONLY; then
    echo "ERROR: Input folder does not exist: $INPUT_FOLDER" >&2
    exit 1
fi

if [[ -z "$OUTPUT_FOLDER" ]]; then
    OUTPUT_FOLDER="$(dirname "${INPUT_FOLDER:-.}")/transcriptions"
fi

mkdir -p "$OUTPUT_FOLDER"

TEMP_DIR="${TEMP_DIR_OVERRIDE:-$OUTPUT_FOLDER/temp}"
ARCHIVE_DIR="$OUTPUT_FOLDER/archive"
LOG_FILE="$OUTPUT_FOLDER/transcription_log.csv"
STATE_FILE="$OUTPUT_FOLDER/transcription_state.csv"

mkdir -p "$TEMP_DIR" "$ARCHIVE_DIR"

[[ ! -f "$LOG_FILE" ]]   && echo "Timestamp,Level,Message"                                            > "$LOG_FILE"
[[ ! -f "$STATE_FILE" ]] && echo "FileName,FileSize,ProcessingTime,Status,Timestamp,PlayerName,ErrorMessage" > "$STATE_FILE"

# ── Logging ───────────────────────────────────────────────────────────────────

log() {
    local level="$1"
    local message="$2"
    local timestamp
    timestamp="$(date '+%Y-%m-%d %H:%M:%S')"
    printf '%s,%s,%s\n' "$timestamp" "$level" "$message" >> "$LOG_FILE"
    case "$level" in
        INFO)    echo "  $message" ;;
        WARNING) echo "  WARNING: $message" ;;
        ERROR)   echo "  ERROR: $message" >&2 ;;
    esac
}

# ── Dependency Check ───────────────────────────────────────────────────────────

check_dependencies() {
    local ok=true

    echo ""
    echo "Checking dependencies..."

    if command -v "$WHISPER_BIN" &>/dev/null; then
        echo "  [OK] $WHISPER_BIN found: $(command -v "$WHISPER_BIN")"
    else
        echo "  [MISSING] $WHISPER_BIN"
        if command -v pacman &>/dev/null; then
            echo "            On Arch, install it rather than building:"
            echo "              sudo pacman -S ggml-vulkan whisper-cpp"
            echo "            Swap ggml-vulkan for ggml-cuda, ggml-hip or ggml-cpu to match your hardware."
            echo ""
        fi
        echo "            Or build it. Upstream is ggml-org/whisper.cpp:"
        echo "              git clone https://github.com/ggml-org/whisper.cpp"
        echo "              cd whisper.cpp"
        echo "              cmake -B build -DGGML_VULKAN=1"
        echo "              cmake --build build -j\$(nproc) --config Release"
        echo "              sudo install -Dm755 build/bin/whisper-cli /usr/local/bin/whisper-cli"
        ok=false
    fi

    if command -v ffmpeg &>/dev/null; then
        echo "  [OK] ffmpeg found"
    else
        echo "  [MISSING] ffmpeg. Install with: $(install_hint ffmpeg)"
        ok=false
    fi

    if [[ -f "$MODEL_PATH" ]]; then
        local size_mb
        size_mb=$(( $(stat -c%s "$MODEL_PATH") / 1048576 ))
        echo "  [OK] Model found: $MODEL_PATH (${size_mb} MB)"
    else
        echo "  [MISSING] Model: $MODEL_PATH"
        echo "            Download with:"
        echo "              mkdir -p \"$(dirname "$MODEL_PATH")\""
        echo "              wget -c -O \"$MODEL_PATH\" \\"
        echo "                https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin"
        echo "            Or, from a whisper.cpp checkout:"
        echo "              ./models/download-ggml-model.sh large-v3-turbo"
        ok=false
    fi

    if command -v jq &>/dev/null; then
        echo "  [OK] jq found"
    else
        echo "  [MISSING] jq. Install with: $(install_hint jq)"
        ok=false
    fi

    if [[ -z "$ALIASES_FILE" ]]; then
        echo "  [WARNING] No campaign selected, so no canon_aliases.json."
        echo "            No Whisper seed prompt, and speaker names fall back to"
        echo "            filename parsing. Pass --campaign SLUG or --aliases PATH."
    elif [[ -f "$ALIASES_FILE" ]]; then
        echo "  [OK] canon_aliases.json found: $ALIASES_FILE"
        if [[ -n "$WHISPER_PROMPT" ]]; then
            echo "  [OK] Whisper seed prompt loaded ($(echo "$WHISPER_PROMPT" | wc -w) words)"
        else
            echo "  [WARNING] seed_prompt missing or empty in $ALIASES_FILE"
        fi
    else
        echo "  [WARNING] canon_aliases.json not found: $ALIASES_FILE"
        echo "            Speaker names will fall back to filename parsing."
    fi

    echo ""
    $ok && return 0 || return 1
}

# ── Player Name / Speaker Resolution ──────────────────────────────────────────
# Craig bot names audio files as: N-discordusername.ext or N-discordusername_M.ext
#
# Priority 1: discord_names lookup in canon_aliases.json.
#   Strips the leading N- track number to get the raw Discord username, then
#   looks it up. Returns the character name for players, "GM" for the GM track.
#   To add a new player or guest, add them to discord_names — no code change needed.
# Priority 2: filename regex N-playername_M.ext  (original Craig convention).
# Priority 3: raw basename (last resort).

get_player_name() {
    local filename="$1"
    local basename="${filename%.*}"   # strip extension

    # Strip leading track number (N-) to get the raw Discord username
    local discord_stem="${basename#*-}"

    if command -v jq &>/dev/null && [[ -f "$ALIASES_FILE" ]]; then
        local char role
        char="$(jq -r --arg k "$discord_stem" \
            '.discord_names[$k].character // empty' "$ALIASES_FILE" 2>/dev/null)"
        role="$(jq -r --arg k "$discord_stem" \
            '.discord_names[$k].role // empty' "$ALIASES_FILE" 2>/dev/null)"
        if [[ -n "$char" ]]; then
            echo "$char"; return
        elif [[ "$role" == "GM" ]]; then
            echo "GM"; return
        fi
    fi

    # Fallback: Match N-playername_M.ext — name between dash and first underscore
    if [[ "$basename" =~ ^[0-9]+-([^_]+)_+[0-9]+$ ]]; then
        echo "${BASH_REMATCH[1]}"
    else
        echo "$basename"
    fi
}

# ── Filler Word Filter ────────────────────────────────────────────────────────

should_filter() {
    local text="$1"
    local stripped="${text%[.,!?;:]}"  # strip one trailing punctuation mark
    local word
    for word in "${IGNORE_WORDS[@]}"; do
        if [[ "${text,,}" == "${word,,}" ]] || [[ "${stripped,,}" == "${word,,}" ]]; then
            return 0   # filter this line
        fi
    done
    return 1
}

# ── Time Formatting (milliseconds → HH:MM:SS) ─────────────────────────────────

format_time() {
    local ms="$1"
    local total_secs=$(( ms / 1000 ))
    local h=$(( total_secs / 3600 ))
    local m=$(( (total_secs % 3600) / 60 ))
    local s=$(( total_secs % 60 ))
    printf "%02d:%02d:%02d" "$h" "$m" "$s"
}

# ── State Recording ───────────────────────────────────────────────────────────

record_state() {
    local filename="$1"
    local file_size="$2"
    local duration_s="$3"
    local status="$4"
    local player_name="$5"
    local error_msg="$6"
    local timestamp
    timestamp="$(date '+%Y-%m-%d %H:%M:%S')"
    printf '"%s","%s","%s","%s","%s","%s","%s"\n' \
        "$filename" "$file_size" "$duration_s" "$status" "$timestamp" "$player_name" "$error_msg" \
        >> "$STATE_FILE"
}

# ── Process Raw Whisper TSV → Filtered/Collapsed processed TSV ────────────────

process_whisper_tsv() {
    local tsv_file="$1"
    local player_name="$2"
    local out_file="$3"
    local filtered_count=0
    local collapsed_count=0

    local prev_text="" prev_start="" prev_end=""
    local first=true

    {
        printf 'speaker\tstart\tend\ttext\n'

        flush_prev() {
            if ! $first; then
                printf '%s\t%s\t%s\t%s\n' "$player_name" "$prev_start" "$prev_end" "$prev_text"
            fi
        }

        while IFS=$'\t' read -r start end text; do
            # Skip header
            [[ "$start" == "start" ]] && continue
            # Skip blank lines
            [[ -z "${start}${end}${text}" ]] && continue
            # Trim leading/trailing whitespace from text
            text="${text#"${text%%[![:space:]]*}"}"
            text="${text%"${text##*[![:space:]]}"}"
            # Strip carriage return (Windows line endings)
            text="${text%$'\r'}"

            # Filter filler words
            if should_filter "$text"; then
                (( filtered_count++ )) || true
                continue
            fi

            if $first; then
                prev_text="$text"
                prev_start="$start"
                prev_end="$end"
                first=false
            elif [[ "${text,,}" == "${prev_text,,}" ]]; then
                # Collapse exact duplicate: extend end timestamp
                prev_end="$end"
                (( collapsed_count++ )) || true
            else
                printf '%s\t%s\t%s\t%s\n' "$player_name" "$prev_start" "$prev_end" "$prev_text"
                prev_text="$text"
                prev_start="$start"
                prev_end="$end"
            fi
        done < "$tsv_file"

        # Flush last entry
        if ! $first; then
            printf '%s\t%s\t%s\t%s\n' "$player_name" "$prev_start" "$prev_end" "$prev_text"
        fi

    } > "$out_file"

    log INFO "  Processed: $(basename "$out_file") — filtered: $filtered_count, collapsed: $collapsed_count"
}

# ── Transcribe One Audio File ─────────────────────────────────────────────────

transcribe_file() {
    local audio_file="$1"
    local filename
    filename="$(basename "$audio_file")"
    local basename="${filename%.*}"
    local player_name
    player_name="$(get_player_name "$filename")"
    local file_size
    file_size="$(stat -c%s "$audio_file" 2>/dev/null || echo 0)"
    local start_time end_time duration_s

    start_time="$(date +%s)"
    log INFO "Transcribing: $filename — speaker: $player_name ($(( file_size / 1048576 )) MB)"

    local raw_json="$TEMP_DIR/${basename}.json"

    # Pre-convert to 16kHz mono WAV before transcription.
    # whisper-cli's built-in miniaudio decoder can fail on certain MP3/OGG variants;
    # ffmpeg handles every format reliably, and 16kHz mono WAV is whisper's native format.
    local wav_file="$TEMP_DIR/${basename}_input.wav"
    log INFO "  Converting to WAV via ffmpeg..."
    if ! ffmpeg -loglevel warning -y \
            -i "$audio_file" \
            -ar 16000 -ac 1 -f wav \
            "$wav_file" \
            2>> "$LOG_FILE"; then
        end_time="$(date +%s)"
        duration_s=$(( end_time - start_time ))
        log ERROR "ffmpeg conversion failed for: $filename"
        record_state "$filename" "$file_size" "$duration_s" "Error" "$player_name" "ffmpeg conversion failed"
        return 1
    fi

    # Run whisper.cpp
    # Flags:
    #   -m      model path
    #   -l      language
    #   -of     output file path without extension (this build uses -of, not -od)
    #   -oj     JSON output (this build has no TSV; JSON has offsets in ms)
    #   -mc 0   no context from previous segments (reduces hallucination on silence)
    #   --prompt  seed Whisper with canon proper nouns from canon_aliases.json
    #
    # Note: -mc 0 can weaken --prompt influence after the first audio segment.
    # If proper noun spellings do not improve, try raising -mc to 64 and re-check
    # that silence hallucination does not creep back. Test on one track first.
    local whisper_args=(-m "$MODEL_PATH" -l en -of "$TEMP_DIR/${basename}" -oj -mc 0)
    [[ -n "$WHISPER_PROMPT" ]] && whisper_args+=(--prompt "$WHISPER_PROMPT")

    if "$WHISPER_BIN" "${whisper_args[@]}" "$wav_file" 2>> "$LOG_FILE"; then

        if [[ ! -f "$raw_json" ]]; then
            end_time="$(date +%s)"
            duration_s=$(( end_time - start_time ))
            log ERROR "Expected JSON not found after transcription: $raw_json"
            record_state "$filename" "$file_size" "$duration_s" "Error" "$player_name" "Output JSON missing"
            return 1
        fi

        # Remove the intermediate WAV now that whisper is done with it
        rm -f "$wav_file"

        # Archive the raw whisper output
        cp "$raw_json" "$ARCHIVE_DIR/${basename}.whisper_original.json"

        # Convert JSON to tab-separated format for processing
        # .offsets.from/.to are in milliseconds; strip leading space from .text
        local raw_tsv="$TEMP_DIR/${basename}.raw.tsv"
        jq -r '.transcription[] | [(.offsets.from | tostring), (.offsets.to | tostring), (.text | ltrimstr(" "))] | @tsv' \
            "$raw_json" > "$raw_tsv"

        # Post-process: filter, collapse, add speaker column
        local processed_tsv="$TEMP_DIR/${basename}.processed.tsv"
        process_whisper_tsv "$raw_tsv" "$player_name" "$processed_tsv"

        end_time="$(date +%s)"
        duration_s=$(( end_time - start_time ))
        record_state "$filename" "$file_size" "$duration_s" "Success" "$player_name" ""
        log INFO "Done: $filename in ${duration_s}s"
        return 0

    else
        rm -f "$wav_file"
        end_time="$(date +%s)"
        duration_s=$(( end_time - start_time ))
        record_state "$filename" "$file_size" "$duration_s" "Error" "$player_name" "whisper-cli exited non-zero"
        log ERROR "whisper-cli failed on: $filename"
        return 1
    fi
}

# ── Aggregate: Merge Processed TSVs Sorted by Timestamp ───────────────────────

create_aggregate_transcripts() {
    local -a processed_files=()
    while IFS= read -r -d '' f; do
        processed_files+=("$f")
    done < <(find "$TEMP_DIR" -maxdepth 1 -name "*.processed.tsv" -print0 | sort -z)

    if [[ ${#processed_files[@]} -eq 0 ]]; then
        log WARNING "No processed TSV files found to aggregate"
        return 1
    fi

    log INFO "Aggregating ${#processed_files[@]} processed files..."

    local merged_file="$OUTPUT_FOLDER/merged_transcript.tsv"

    {
        printf 'speaker\tstart\tend\ttext\n'
        for f in "${processed_files[@]}"; do
            tail -n +2 "$f"
        done | sort -t$'\t' -k2,2n
    } > "$merged_file"

    log INFO "Created merged_transcript.tsv"

    # Per-speaker files in temp/
    local speaker
    while IFS= read -r speaker; do
        [[ -z "$speaker" ]] && continue
        local speaker_file="$TEMP_DIR/${speaker}_transcript.tsv"
        {
            printf 'speaker\tstart\tend\ttext\n'
            awk -F'\t' -v sp="$speaker" '$1 == sp' "$merged_file"
        } > "$speaker_file"
        log INFO "Speaker file: temp/${speaker}_transcript.tsv"
    done < <(awk -F'\t' 'NR>1 && $1!="" {print $1}' "$merged_file" | sort -u)
}

# ── Consolidate: Merge Consecutive Same-Speaker Lines ─────────────────────────

create_consolidated_transcript() {
    local merged_file="$OUTPUT_FOLDER/merged_transcript.tsv"
    local consolidated_file="$OUTPUT_FOLDER/consolidated_transcript.tsv"

    if [[ ! -f "$merged_file" ]]; then
        log ERROR "merged_transcript.tsv not found; cannot consolidate"
        return 1
    fi

    log INFO "Consolidating consecutive same-speaker lines..."

    local prev_speaker="" prev_start="" prev_end="" prev_text=""
    local first=true
    local original_count=0 consolidated_count=0

    {
        printf 'speaker\tstart\tend\ttext\n'

        while IFS=$'\t' read -r speaker start end text; do
            [[ "$speaker" == "speaker" ]] && continue
            [[ -z "$speaker" ]] && continue
            (( original_count++ )) || true

            if $first; then
                prev_speaker="$speaker"
                prev_start="$start"
                prev_end="$end"
                prev_text="$text"
                first=false
            elif [[ "$speaker" == "$prev_speaker" ]]; then
                prev_end="$end"
                prev_text="$prev_text $text"
            else
                printf '%s\t%s\t%s\t%s\n' "$prev_speaker" "$prev_start" "$prev_end" "$prev_text"
                (( consolidated_count++ )) || true
                prev_speaker="$speaker"
                prev_start="$start"
                prev_end="$end"
                prev_text="$text"
            fi
        done < "$merged_file"

        if ! $first; then
            printf '%s\t%s\t%s\t%s\n' "$prev_speaker" "$prev_start" "$prev_end" "$prev_text"
            (( consolidated_count++ )) || true
        fi

    } > "$consolidated_file"

    local reduction=0
    if [[ $original_count -gt 0 ]]; then
        reduction=$(( (original_count - consolidated_count) * 100 / original_count ))
    fi

    log INFO "Created consolidated_transcript.tsv ($original_count → $consolidated_count lines, ${reduction}% reduction)"
}

# ── Markdown Transcript ────────────────────────────────────────────────────────

create_markdown_transcript() {
    local consolidated_file="$OUTPUT_FOLDER/consolidated_transcript.tsv"
    local md_file="$OUTPUT_FOLDER/transcript.md"

    if [[ ! -f "$consolidated_file" ]]; then
        log ERROR "consolidated_transcript.tsv not found; cannot create markdown"
        return 1
    fi

    log INFO "Creating transcript.md..."

    local entry_count=0

    {
        while IFS=$'\t' read -r speaker start end text; do
            [[ "$speaker" == "speaker" ]] && continue
            [[ -z "$speaker" ]] && continue
            local ts
            ts="$(format_time "$start")"
            printf '**[%s] %s:**  \n%s\n\n' "$ts" "$speaker" "$text"
            (( entry_count++ )) || true
        done < "$consolidated_file"
    } > "$md_file"

    log INFO "Created transcript.md ($entry_count entries)"
}

# ── Stats Summary ──────────────────────────────────────────────────────────────

print_stats() {
    local total success errors
    total="$(awk -F',' 'NR>1' "$STATE_FILE" 2>/dev/null | wc -l)"
    success="$(grep -c '"Success"' "$STATE_FILE" 2>/dev/null || echo 0)"
    errors="$(grep -c '"Error"' "$STATE_FILE" 2>/dev/null || echo 0)"

    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "  Transcription Summary"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    printf  "  Total processed : %s\n" "$total"
    printf  "  Success         : %s\n" "$success"
    printf  "  Errors          : %s\n" "$errors"
    echo    "  Output folder   : $OUTPUT_FOLDER"
    echo ""
    echo "  Output files:"
    [[ -f "$OUTPUT_FOLDER/merged_transcript.tsv" ]]      && echo "    merged_transcript.tsv"
    [[ -f "$OUTPUT_FOLDER/consolidated_transcript.tsv" ]] && echo "    consolidated_transcript.tsv"
    [[ -f "$OUTPUT_FOLDER/transcript.md" ]]               && echo "    transcript.md"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo ""
}

# ══ MAIN ══════════════════════════════════════════════════════════════════════

# --check mode: just verify dependencies and exit
if $CHECK_ONLY; then
    check_dependencies
    exit $?
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  Session Transcription${CAMPAIGN:+ — $CAMPAIGN${SESSION:+ S$SESSION}}"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
log INFO "Campaign : ${CAMPAIGN:-(none)}"
log INFO "Aliases  : ${ALIASES_FILE:-(none)}"
log INFO "Input    : $INPUT_FOLDER"
log INFO "Output   : $OUTPUT_FOLDER"
log INFO "Temp     : $TEMP_DIR"
log INFO "Model    : $MODEL_PATH"
log INFO "Force    : $FORCE | PostOnly: $POST_PROCESS_ONLY | Cleanup: $CLEANUP"
log INFO "Filter   : ${IGNORE_WORDS[*]}"

# ── Transcription Phase ────────────────────────────────────────────────────────

if ! $POST_PROCESS_ONLY; then

    check_dependencies || exit 1

    # Load already-processed filenames from state file (avoid re-running unless --force)
    declare -a processed_list=()
    if [[ -f "$STATE_FILE" ]]; then
        while IFS=',' read -r fname _rest; do
            fname="${fname//\"/}"
            [[ "$fname" == "FileName" ]] && continue
            [[ "$_rest" == *'"Success"'* ]] && processed_list+=("$fname")
        done < "$STATE_FILE"
    fi

    # Find audio files
    mapfile -t audio_files < <(
        find "$INPUT_FOLDER" -maxdepth 1 -type f \( \
            -iname "*.mp3"  -o -iname "*.wav"  -o -iname "*.m4a"  -o \
            -iname "*.flac" -o -iname "*.ogg"  -o -iname "*.aac"  -o \
            -iname "*.mp4"  -o -iname "*.wma" \
        \) | sort
    )

    if [[ ${#audio_files[@]} -eq 0 ]]; then
        log WARNING "No supported audio files found in $INPUT_FOLDER"
        exit 0
    fi

    log INFO "Found ${#audio_files[@]} audio file(s)"
    echo "PROGRESS:start:${#audio_files[@]}"
    echo ""

    file_idx=0
    for audio_file in "${audio_files[@]}"; do
        fname="$(basename "$audio_file")"
        (( file_idx++ )) || true
        echo "PROGRESS:file:${file_idx}/${#audio_files[@]}:${fname}"

        if ! $FORCE; then
            already_done=false
            for pf in "${processed_list[@]}"; do
                [[ "$pf" == "$fname" ]] && { already_done=true; break; }
            done
            if $already_done; then
                log INFO "Skipping (already processed): $fname"
                echo "PROGRESS:done:${file_idx}/${#audio_files[@]}:${fname}"
                continue
            fi
        fi

        transcribe_file "$audio_file" || true   # continue on per-file errors
        echo "PROGRESS:done:${file_idx}/${#audio_files[@]}:${fname}"
        echo ""
    done
    echo "PROGRESS:complete"

else
    # ── Post-Process Only Mode ──────────────────────────────────────────────

    log INFO "Post-process only — re-processing archived whisper output"

    # Remove stale .processed.tsv files so we don't mix old and new
    find "$TEMP_DIR" -maxdepth 1 -name "*.processed.tsv" -delete

    declare -a source_files=()
    declare -A seen_basenames=()

    # Prefer archive JSON files as the canonical source of truth
    while IFS= read -r -d '' f; do
        bn="$(basename "$f" .whisper_original.json)"
        source_files+=("$f")
        seen_basenames["$bn"]=1
    done < <(find "$ARCHIVE_DIR" -maxdepth 1 -name "*.whisper_original.json" -print0)

    # Add raw JSONs from temp/ only for files not already in archive
    while IFS= read -r -d '' f; do
        bn="$(basename "$f" .json)"
        if [[ -z "${seen_basenames[$bn]+x}" ]]; then
            source_files+=("$f")
            seen_basenames["$bn"]=1
        fi
    done < <(find "$TEMP_DIR" -maxdepth 1 -name "*.json" -print0)

    if [[ ${#source_files[@]} -eq 0 ]]; then
        log WARNING "No whisper JSON files found in archive/ or temp/"
        exit 0
    fi

    log INFO "Found ${#source_files[@]} whisper JSON file(s) to reprocess"

    for f in "${source_files[@]}"; do
        fname="$(basename "$f")"
        # Derive original base name
        orig_base="${fname%.whisper_original.json}"
        orig_base="${orig_base%.json}"
        player_name="$(get_player_name "$orig_base")"
        # Convert JSON → raw TSV, then post-process
        raw_tsv="$TEMP_DIR/${orig_base}.raw.tsv"
        jq -r '.transcription[] | [(.offsets.from | tostring), (.offsets.to | tostring), (.text | ltrimstr(" "))] | @tsv' \
            "$f" > "$raw_tsv"
        processed_tsv="$TEMP_DIR/${orig_base}.processed.tsv"
        log INFO "Reprocessing: $fname → speaker: $player_name"
        process_whisper_tsv "$raw_tsv" "$player_name" "$processed_tsv"
    done
fi

# ── Post-Processing Pipeline ───────────────────────────────────────────────────

echo ""
log INFO "Running post-processing pipeline..."

create_aggregate_transcripts   || log WARNING "Aggregation step failed"
create_consolidated_transcript || log WARNING "Consolidation step failed"
create_markdown_transcript     || log WARNING "Markdown step failed"
echo "PROGRESS:postdone"

# ── Cleanup ────────────────────────────────────────────────────────────────────

if $CLEANUP; then
    log INFO "Removing temp/ directory"
    rm -rf "$TEMP_DIR"
fi

print_stats
