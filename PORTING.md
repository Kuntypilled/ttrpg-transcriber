# PORTING.md: ttrpg_transcriber → Rust TUI

Spec for rebuilding the session transcriber as a Rust TUI (ratatui + crossterm,
mouse support, yazi/rmpc feel). Written from a full read of the Python/bash
source at commit `98aa387` (2026-10-11). Everything below was read off the code,
not run. Where behaviour is a guess, it says so.

The working GTK GUI stays on `main` of the original repo until this replaces it.
Do not edit that repo to build this one.

---

## 0. Decisions already made

| Decision | Choice |
|---|---|
| Frontend | Rust TUI, ratatui + crossterm, mouse enabled |
| Replace or coexist | Replace the GUI once the TUI is verified working |
| Why Rust | Polish and snappiness (instant start, never block the UI). Not for throughput: the heavy work is whisper-cli / ffmpeg / embeddings, which are native already |
| Where it lives | New repo. Original repo untouched |
| ChromaDB / embeddings | **Stay Python.** Called as a subprocess. Not worth a Rust rewrite (see section 6) |

Settle these at the start of the planning pass (section 10) before writing code.

---

## 1. What the tool does (one paragraph)

A GM records a TTRPG session with the Craig Discord bot, which produces one
audio file per speaker. The tool: picks a campaign, moves the audio into a
per-session folder, transcribes each track with whisper.cpp, labels each track
with a speaker from the campaign's `canon_aliases.json`, filters filler words,
merges all tracks by timestamp into `transcript.md`, then (after the GM's manual
recap pass) chunks the transcript into ~280-word windows with metadata and
ingests them into a per-campaign ChromaDB for later semantic search.

```
audio files ──► [transcribe.sh] ──► transcript.md ──► (human recap pass)
                                                           │
   canon_aliases.json feeds every stage                    ▼
                                         [transcript_to_chunks.py] ──► s<nn>_chunks.jsonl
                                                           │
                                                           ▼
                                              [chroma_memory.py ingest] ──► Chroma DB
```

---

## 2. Source inventory

Original repo, all in one directory. Line counts at `98aa387`.

| File | Lines | Role | Port plan |
|---|---|---|---|
| `campaign.py` | 789 | Config + campaign registry + all path rules + JSON splice | Rewrite in Rust (section 3) |
| `transcribe.sh` | 867 | Whisper pipeline, speaker labelling, TSV/markdown generation | Rewrite in Rust (section 4) |
| `transcript_to_chunks.py` | 413 | Parse, alias-clean, window, JSONL; plus alias-candidate report | Rewrite in Rust (section 5) |
| `chroma_memory.py` | 177 | ChromaDB ingest / query / info | **Keep Python** (section 6) |
| `transcribe_gui.py` | ~1400 | GTK3 UI and orchestration | Replaced by the TUI (section 7) |
| `launch_transcriber.sh`, `.desktop.example` | | Launchers for the GUI | Replace with a plain binary + optional desktop entry |
| `layout-*.sh` | | Terminal layout helpers, unrelated to the pipeline | Ignore |

Dependencies the pipeline shells out to (all stay external):
`whisper-cli` (whisper.cpp), `ffmpeg`, and `jq` (**drop jq**: serde_json replaces it).
Default model: `~/.local/share/whisper.cpp/models/ggml-large-v3-turbo.bin`.

---

## 3. Configuration and paths (`campaign.py`)

This module is the single source of truth for every path. Port it first; the rest
depends on it. Preserve behaviour exactly, because the paths point at existing
data (transcripts in the vault, an existing Chroma store).

### 3.1 Config resolution order, for every root

1. environment variable
2. config file: `$GM_TRANSCRIBER_CONFIG`, else `$XDG_CONFIG_HOME/ttrpg-transcriber/config.json`
3. built-in default (only where a correct one exists)

`XDG_*` values are honoured only if absolute; otherwise `~/.config`,
`~/.local/share`, `~/.cache`, `~/.local/state`.

| Setting | Env vars (first wins) | Config key | Default |
|---|---|---|---|
| Vaults root (one vault per world) | `GM_VAULTS_ROOT` | `vaults_root` | none, deliberately |
| Single vault | `GM_VAULT`, `VAULT` | `vault` | none, deliberately |
| Audio root | `GM_AUDIO_ROOT` | `audio_root` | `$XDG_DATA_HOME/ttrpg_audio` |
| Chroma root | `GM_CHROMA_ROOT` | `chroma_root` | `$XDG_DATA_HOME/ttrpg_memory` |
| Work root | `GM_WORK_ROOT` | `work_root` | `$XDG_CACHE_HOME/ttrpg_transcribe` |
| Campaigns subdir | `GM_CAMPAIGNS_SUBDIR` | `campaigns_subdir` | `0.2_campaigns` |
| Transcripts subdir | `GM_TRANSCRIPTS_SUBDIR` | `transcripts_subdir` | `08_transcripts` |
| Whisper GPU index | `GM_WHISPER_DEVICE` | `whisper_device` | unset (whisper-cli default) |

- If both a vaults root and a single vault are configured, **vaults root wins**.
- No vault default, on purpose: a wrong guess fails silently (empty campaign
  list). Unconfigured must produce the `UNCONFIGURED` instructions text, not an
  empty list. Copy that message.
- Every value gets `~` expanded and made absolute.
- `whisper_device` must be an integer ≥ 0, otherwise a config error naming the source.
- A config file that is not valid JSON is an error naming the file.
- `config.json` may contain `_comment` keys. Ignore any key it doesn't know.
- Must remember **where each value came from** (env var name / config path /
  "built-in default"): the `doctor` command prints it.

### 3.2 Campaign discovery

A campaign is any folder holding a `canon_aliases.json`.

- Multi-vault: glob `<vaults_root>/*/<campaigns_subdir>/*/canon_aliases.json`,
  id = `<world>/<campaign>`.
- Single vault: glob `<vault>/<campaigns_subdir>/*/canon_aliases.json`,
  id = `<campaign>`.
- Sorted by path. An unreadable or invalid file is **skipped**, not fatal.
- Unconfigured vault yields an empty list, and the UI shows `setup_hint()`
  instead of dying.
- Campaign id validation: multi-vault requires exactly `world/campaign`;
  single-vault requires exactly one segment; reject `""`, `.`, `..` segments
  (path traversal guard). Port this faithfully.

### 3.3 Path rules

`<c>` = campaign folder, `<id>` = campaign id, `<nn>` = session zero-padded to
2 digits (`S07`, `S13`; session 123 → `S123`).

| Thing | Path |
|---|---|
| Aliases file | `<c>/canon_aliases.json` |
| Transcripts dir | `<c>/<transcripts_subdir>/` |
| Session dir | `<transcripts>/S<nn>/` |
| transcript.md | `<transcripts>/S<nn>/transcript.md` |
| Chunks dir | `<transcripts>/chunks/` |
| Chunks file | `<transcripts>/chunks/s<nn>_chunks.jsonl` (lowercase `s`) |
| Audio base | campaign `audio_root` override if set in its JSON, else `<audio_root>/<id>` |
| Audio dir | `<audio_base>/Session <n>` (**not** zero-padded, with a space) |
| Chroma DB | `<chroma_root>/<id>/` |
| Work dir | `<work_root>/<id>/S<nn>/` (whisper temp lives at `.../temp`) |

Audio, Chroma and work files are deliberately **outside the vault** (Obsidian
Sync would push megabytes per write). Archive JSON stays **inside** the vault:
it is the one artifact you cannot regenerate once the audio is deleted.

### 3.4 `canon_aliases.json` schema

One file per campaign. Hand-maintained; its formatting is load-bearing.

| Key | Type | Used by |
|---|---|---|
| `campaign` | string | display label (also `display_name` takes precedence) |
| `world` | string | informational |
| `system` | string | shown in menu label |
| `seed_prompt` | string | passed to `whisper-cli --prompt` |
| `aliases` | `{canon: [variants]}` | SAFE substitutions, auto-applied by chunker |
| `review_only` | `{canon: [variants]}` | manglings that collide with English words; **never** auto-applied |
| `names` | `[string]` | canon proper nouns, for the alias report |
| `pc_nicknames` | `{canon: [nicknames]}` | alias report |
| `speaker_map` | `[{match, player, character}]` | ordered, first match wins |
| `players` | `{character: player}` | **key order defines `has_<PC>` metadata order** |
| `discord_names` | `{stem: {player, character, role}}` | audio filename → speaker |
| `audio_root` | string | per-campaign audio location override |

Rules to preserve:
- Keys starting with `_` are comments. Skip them everywhere (`aliases`,
  `review_only`, `players`, `discord_names`; and rows in `speaker_map` lacking `match`).
- `normalize_character`: trim, collapse whitespace runs to `_`. `"Kestrel Vane"`
  → `"Kestrel_Vane"`. `null` stays null.
- `speaker_map` fallback: if absent or empty, **derive** one from `players` then
  from `discord_names` GM entries, then append `("gm", null, null)` if no `gm`
  key. Port `_derived_speaker_map` exactly, including de-duplication by key.
- `players` fallback: if absent, derive from `speaker_map` characters in
  first-seen order.
- `has_review_only` = `review_only` non-empty after dropping comment keys.
- `alias_subs`: for each canon, for each variant, in **file order**, a
  case-insensitive regex `\b<escaped variant>\b` replaced by canon. Order
  matters when variants overlap. **Requires serde_json `preserve_order`** (or
  equivalent), otherwise the order is lost.

### 3.5 Surgical JSON append (`append_object_entries`)

Used when the GM resolves unknown speakers. It must **not** reformat the file:
key order, column alignment and `_comment` fields are documentation.

Algorithm (port as-is, it is careful):
1. Read text; parse to validate; confirm `key` is an object.
2. Drop entries whose key already exists (never overwrite).
3. Find the key's `{ ... }` span by brace matching that respects strings and
   escapes.
4. Append `\n<indent><json key>: <json value>,` for each new entry, strip the
   last comma, keep original trailing whitespace.
5. **Verify**: new text parses; every pre-existing top-level key and every
   pre-existing entry under `key` is unchanged. Otherwise abort without writing.
6. Write the file; return the keys actually added.

Do this with text splicing, not serde round-tripping. Add a test that diffs the
file before/after and asserts only the appended lines changed.

### 3.6 CLI surface to keep (during transition)

`campaign.py list | doctor | whisper-device | paths --campaign ID [--session N]`.
`paths` prints `KEY=value` (single-quoted) for `CAMPAIGN, CAMPAIGN_LABEL,
CAMPAIGN_SYSTEM, CAMPAIGN_WORLD, CAMPAIGN_ROOT, ALIASES_FILE, TRANSCRIPTS_DIR,
CHUNKS_DIR, AUDIO_BASE, DB_PATH` and with a session `SESSION, SESSION_DIR,
TRANSCRIPT_MD, CHUNKS_FILE, AUDIO_DIR, WORK_DIR`. The Rust binary should expose
`doctor` and `list` equivalents; `paths` only matters while any bash remains.

---

## 4. Transcription pipeline (`transcribe.sh`)

### 4.1 Inputs

`--campaign ID --session N` (campaign mode) or `-i DIR` (manual). Options:
`-o`, `--aliases`, `--temp-dir`, `-m/--model`, `-w/--whisper-bin` (default
`whisper-cli`), `-d/--device`, `-f/--force`, `-p/--post-process-only`,
`-c/--cleanup`, `--ignore-words a,b,c` (replaces defaults), `--check`.

Defaults: ignore words `you silence um uh ah like right well`.

### 4.2 Per-run setup

- Output folder = session dir; temp = `<work_dir>/temp`; archive =
  `<output>/archive`; both created.
- Files created if missing:
  - `transcription_log.csv`, header `Timestamp,Level,Message`
  - `transcription_state.csv`, header
    `FileName,FileSize,ProcessingTime,Status,Timestamp,PlayerName,ErrorMessage`
- Whisper prompt = `seed_prompt` from the aliases file (empty if absent).
- The GUI passes `-i audio_dir -o trans_dir` explicitly because it moves the
  audio itself first. In the port, one component owns the move.

### 4.3 Dependency check (`--check`)

Verifies `whisper-cli` on PATH, `ffmpeg`, the model file, `jq` (drop), and the
aliases file / seed prompt. Prints distro-specific install hints (pacman / apt /
dnf / zypper). Exit non-zero when a hard dependency is missing; a missing
aliases file is a **warning**, not a failure. The TUI should run this at startup
and show a clear banner instead of failing mid-run. `--check` must not require
the audio folder to exist (recent fix, commit `700e357`).

### 4.4 Per-file transcription

1. Skip unless `--force` when the filename already has a `"Success"` row in the
   state file (whole-line match on the first CSV field).
2. Resolve speaker (4.5).
3. `ffmpeg -loglevel warning -y -i <audio> -ar 16000 -ac 1 -f wav <temp>/<base>_input.wav`
   (whisper's built-in decoder fails on some MP3/OGG; ffmpeg is the reliable path).
4. `whisper-cli -m <model> -l en -of <temp>/<base> -oj -mc 0 [--prompt <seed>] [-dev N] <wav>`
   - `-mc 0` = no context from previous segments (reduces hallucination on
     silence), at the cost of weaker prompt influence after the first segment.
     The script's own comment suggests trying `-mc 64` if proper nouns don't
     improve. Make this a setting.
5. Expect `<temp>/<base>.json`. Copy to `<archive>/<base>.whisper_original.json`.
   Delete the WAV.
6. Parse JSON: `.transcription[]` → `(offsets.from ms, offsets.to ms, text with
   one leading space trimmed)`.
7. Post-process (4.6) to `<temp>/<base>.processed.tsv`.
8. Append a state row (`Success` or `Error` with a message). Errors on one file
   **do not stop the run**.

whisper-cli stderr goes to the log file, not the UI stream.

### 4.5 Speaker resolution (`get_player_name`)

Input: audio filename, e.g. `2-someuser_0.flac`.

1. Strip extension → `2-someuser_0`.
2. Strip leading track number: drop everything up to and including the **first**
   `-` → `someuser_0`.
3. Look that up in `discord_names`. If `.character` is non-empty → that string
   (raw, **not** underscore-normalized). Else if `.role == "GM"` → `GM`.
4. Fallback regex `^[0-9]+-([^_]+)_+[0-9]+$` → capture group 1.
5. Last resort: the basename.

> **Discrepancy to resolve.** The comment in `canon_aliases.example.json` says
> `2-someuser_0.flac` "looks up `someuser`" (trailing `_M` stripped), but the
> code in both `transcribe.sh` and the GUI looks up `someuser_0`. By the code,
> a `discord_names` key must include the `_0` unless the Craig filename has none.
> Check against a real Craig export before porting. Pick one behaviour on
> purpose and document it. Don't copy the bug by accident.

The GUI re-implements this (`_discord_stem`, `_resolve_discord_names`) for
display. The port must have **one** function used by both the pipeline and the UI.

### 4.6 Filtering and collapsing (`process_whisper_tsv`)

Per track, in order, over `(start, end, text)`:
- trim whitespace, strip trailing `\r`
- skip empty lines
- **filter**: drop if the text equals an ignore word (case-insensitive), either
  exactly or after stripping **one** trailing `.,!?;:`
- **collapse**: if text equals the previous kept text (case-insensitive), extend
  the previous `end` instead of emitting a row
- emit `speaker \t start \t end \t text`, with header `speaker\tstart\tend\ttext`
- log filtered and collapsed counts

### 4.7 Aggregation

1. **merged_transcript.tsv**: all `*.processed.tsv` rows, stable-sorted
   **numerically by `start` (ms)**, header kept. (Original uses
   `sort -k2,2n`; ties are broken by sort's last-resort comparison, so don't
   promise a specific tie order.)
2. Per-speaker TSVs in temp (`<speaker>_transcript.tsv`), a by-product, low value.
3. **consolidated_transcript.tsv**: merge consecutive rows with the same speaker;
   `end` = last end, `text` = space-joined. Log `original → consolidated`
   counts and percentage reduction.
4. **transcript.md**: per consolidated row,
   `**[HH:MM:SS] Speaker:**  \n<text>\n\n` (two trailing spaces after the
   colon-bold, then newline). Timestamp from `start` ms, floored to seconds,
   `HH` not wrapped at 24.

transcript.md format is a **contract**: the chunker's regex parses it
(section 5.1) and the GM's recap pass edits it by hand.

### 4.8 Post-process-only (`-p`)

Replays archived JSON without audio: delete stale `*.processed.tsv`, prefer
`archive/*.whisper_original.json`, add `temp/*.json` for basenames not already
archived, re-run 4.5 to 4.7. Valuable: lets the GM fix `discord_names` and
regenerate. Surface it in the TUI (the GUI doesn't).

### 4.9 Machine-readable progress protocol

stdout lines the GUI parses, colon-separated:

| Line | Meaning |
|---|---|
| `PROGRESS:start:<N>` | N audio files found |
| `PROGRESS:file:<i>/<N>:<filename>` | starting file i |
| `PROGRESS:done:<i>/<N>:<filename>` | file i finished (also emitted for **skipped** files) |
| `PROGRESS:complete` | transcription loop finished |
| `PROGRESS:postdone` | merge/consolidate/markdown finished |

In Rust this becomes a typed event channel
(`enum PipelineEvent { Start(n), FileStart{i,n,name}, FileDone{..}, Log(String), ... }`),
not text parsing. Keep an `--events` machine mode only if you want the old GUI
to keep working against the new binary.

GUI progress mapping, for reference: 0.04 moving files; 0.08 start;
`0.08 + 0.82·(i−1)/N` at file start, `0.08 + 0.82·i/N` at done; 0.92 post-process;
1.0 complete. A single whisper run has no sub-file progress, so a long track
looks frozen. A reason to consider reading whisper-cli's own progress output
(`-pp`/stderr) in the port. Unverified, check whisper-cli's flags first.

---

## 5. Chunker and alias report (`transcript_to_chunks.py`)

### 5.1 Parse transcript.md

Regex (DOTALL):
```
\*\*\[(\d{2}):(\d{2}):(\d{2})\]\s+([^:]+):\*\*\s*\n(.*?)(?=\n\*\*\[|\Z)
```
Groups: hh, mm, ss, speaker label, body. Body trimmed; empty bodies dropped.
Note: the human recap pass edits this file, so the parser should be tolerant,
and a row that fails to match is silently ignored. Consider warning on that.

### 5.2 Speaker resolution (`make_resolve`)

For label `L`, `low = L.lower()`:
- `role = "GM"` if `"gm" in low` (substring, so any speaker label containing
  `gm` becomes GM), else `"Player"`.
- Iterate `speaker_map` in order; first row whose `match` is a substring of
  `low` wins → `(role, player, character)`; if that row's character is null,
  role is forced to `"GM"`.
- No match → `(role, None, None)`.
- `label` for output = character, or `"GM"` when character is null.

### 5.3 Cleaning

Apply `alias_subs` (aliases only, **never** `review_only`) to each utterance in
order, case-insensitive, word-bounded.

### 5.4 Windowing

`target_words = 280`, `overlap = 1` utterance.
```
i = 0
while i < n:
    take utterances from i until word count >= 280
    chunk = those utterances
    if reached end: break
    i = max(j - overlap, i + 1)     # step back by overlap, always advance
```
Chunk text: one line per utterance, `[HH:MM:SS] <label>: <text>`.
Chunk fields: `characters` = sorted unique non-null chars, `has_gm`,
`start` = first ts, `end` = last ts, `words`, `n_utt`.

### 5.5 JSONL output (one object per line)

```json
{"id": "s13_c000",
 "document": "[00:01:02] Vashti_Corrow: ...\n...",
 "metadata": {"session": 13, "date": "2026-08-05", "start": "00:01:02",
              "end": "00:07:40", "has_gm": true, "n_utt": 14,
              "characters": "Bram_Ollander,Vashti_Corrow",
              "has_Vashti_Corrow": true, "has_Bram_Ollander": true}}
```
- id: `s{session:02d}_c{k:03d}`.
- `characters`: comma-joined, sorted.
- One `has_<PC>` boolean per entry of `players`, **in `players` key order**
  (this is why that order is load-bearing).
- `date` defaults to today (local) if not given. The GUI always passes today.
- stderr summary line: `N chunks from M utterances (session S); K alias rules applied`.

Changing chunk shape invalidates the existing Chroma index. Keep the format
byte-compatible until you decide to re-ingest everything.

### 5.6 Alias-candidate report (`--report`)

Read-only. Writes `<session_dir>/alias_candidates.md`, never touches
`canon_aliases.json`. Purpose: seed `review_only` **before** the first ingest.

Algorithm:
1. Words = regex `[A-Za-z][A-Za-z'’\-]*` over utterance bodies.
2. Dictionary from the first existing of `/usr/share/dict/words`,
   `american-english`, `british-english`. If none exists, `is_english` is
   always false (degrades quietly: everything lands in "safe").
3. Canon terms = `names + pcs + aliases keys + review_only keys`, `_`→space,
   length ≥ 3, de-duplicated case-insensitively.
4. `known` = canon terms + all alias/review_only variants + nicknames + speaker
   labels. Anything in `known` is not a candidate.
5. n-gram pools for n = 1..min(max canon word count, 3); grams ≥ 4 chars after
   dropping a trailing possessive `'s`/`’s`.
6. **Section A (near-misses)**: for each canon term vs each gram of the same
   length, `SequenceMatcher.ratio() ≥ threshold` (default 0.70), using
   `real_quick_ratio` and `quick_ratio` as cheap pre-filters. Each variant is
   kept only against its single best-scoring canon term. Split by whether every
   word is a dictionary word: **A1** (not English → safe for `aliases`),
   **A2** (English → `review_only` only).
7. **Section B**: capitalised tokens ≥ 4 chars, not in dictionary, not known,
   not already a near-miss, with count ≥ `min_freq` (default 2). Top 60.
8. A "Ready to paste" section with two JSON blocks (`aliases`, `review_only`).

> **Fidelity warning.** `difflib.SequenceMatcher.ratio()` is the
> Ratcliff/Obershelp measure, not Levenshtein. The 0.70 threshold was tuned
> against it. Using `strsim`'s Levenshtein or Jaro-Winkler will give different
> scores and different candidates. Either port Ratcliff/Obershelp (a crate
> that ports Python's `difflib` exists; verify before depending on it) or
> re-tune the threshold on real data.

Performance note: this is O(canon × grams) with a quadratic similarity. It is
fast enough in Python at campaign scale; a Rust port should be much faster, but
keep the cheap pre-filters.

---

## 6. ChromaDB layer (`chroma_memory.py`)

**Keep as Python.** ChromaDB and sentence-transformers have no practical Rust
equivalent that reads the existing store. A Rust rewrite would mean a different
vector store and a full re-embed, which is a separate project.

### 6.1 The coupling you must resolve

`chroma_memory.py` does `import campaign as campaign_mod` and calls
`campaign_mod.load(...)` to find the DB path, PC list and display info. So the
Rust app **cannot** drop `campaign.py` while it uses this helper. Options:

1. **Vendor both** (`campaign.py` + `chroma_memory.py`) into the new repo as a
   `python/` helper directory. Simplest; two Python files of path logic stay
   duplicated with the Rust code.
2. **Make the helper self-contained**: add `--db PATH` (already supported) and
   `--pcs a,b,c`, and remove the `campaign` import. Rust passes everything.
   Cleaner boundary, small edit to `chroma_memory.py`, and `info`/`ingest`/`query`
   no longer need a campaign id at all.

Recommendation: option 2. The Rust binary already knows every path, so passing
`--db` explicitly means one source of truth. Decide in planning.

### 6.2 Behaviour to preserve

- Collection name `transcripts`, cosine space via
  `configuration={"hnsw": {"space": "cosine"}}`.
- Embedding model `BAAI/bge-base-en-v1.5` (override env `TTRPG_EMBED`).
  **Never mix models in one collection.** Changing the model invalidates the DB;
  the JSONL is what survives.
- `ingest`: read JSONL, `upsert` in batches of 100 (re-ingest overwrites, no
  duplicates). Prints `Upserted N chunks into <db>. Collection now holds M.`
- `query`: filters `session >= --from`, `has_<normalized char> == true`,
  optional `where_document $contains`; `-k` default 6; prints
  `=== s<session> <start>-<end> [<characters>] cos_dist=<d> ===` then the first
  600 chars.
- `info`: db path, exists, embedding model, PCs, chunk count, sessions.
- `$TTRPG_DB` overrides the DB path; `--db` beats it.
- One DB **per campaign** (not one DB with a campaign field), on purpose: a
  metadata filter can be forgotten, a separate DB cannot.
- DB lives outside the vault. It is fully regenerable from the JSONL.
- Heavy startup: importing chromadb + loading the embedding model takes seconds.
  Run it as a background job with a spinner. This is the one place the TUI can't
  be "instant", and that is fine.

### 6.3 Subprocess contract

Rust spawns `python3 chroma_memory.py ingest ...`, streams combined
stdout+stderr as log lines, treats non-zero exit as failure. Resolve the Python
interpreter from a config key (`python`), because ChromaDB usually lives in a
venv or `--user` install. The GUI uses `sys.executable`, which has no analogue
in Rust: this setting is new work.

---

## 7. UI behaviour to carry over (`transcribe_gui.py`)

This is the behaviour spec for the TUI. The GTK layout itself is not carried over.

### 7.1 Screens / regions

- **Header**: title, subtitle `label · system · world`.
- **Campaign picker**: list of campaigns, each with `menu_label`
  (`label — system`) and a sub-line `slug · PC1, PC2`. Selected one highlighted.
  The GUI uses a popover only to dodge an XWayland menu bug; a plain list pane
  makes that workaround moot.
- **Path summary** for the selected campaign (the GUI shows these as a tooltip):
  slug, aliases path, audio, transcripts, chunks, chroma db, characters.
- **Session number**: digits only; `->  S<nn>` preview.
- **Audio files**: chosen files, each shown as `filename → Character (Player)`,
  `filename → GM (Player)`, or `filename → ⚠ Unknown`. Count line:
  `N files selected — K unknown speaker(s)` or `— all speakers resolved`.
- **Review banner** (when campaign has no `review_only`): offers the alias
  report, text differs depending on whether any transcribed session exists.
- **Progress bar + status line**.
- **Log pane** (monospace, autoscroll).
- **Actions**: Choose files, Resolve speakers, Start, Open folder, Index to Memory.

### 7.2 Button enablement rules

| Action | Enabled when |
|---|---|
| Start | campaign selected **and** session > 0 **and** ≥ 1 file chosen |
| Open folder | session dir exists (and after a successful run) |
| Index to Memory | `transcript.md` exists for the session |
| Resolve speakers | ≥ 1 unknown speaker **and** campaign selected |
| Seed review report | campaign lacks `review_only` **and** a transcribed session exists |

Start, Index, and the session field are disabled while a pipeline runs, and
re-enabled in a `finally`, including on error.

### 7.3 Flows

**Choose files**: multi-select, filter `flac mp3 wav m4a ogg aac wma`, start in
`~/Downloads` if it exists, sorted results. (TUI: a file-tree pane, yazi-style.)

**Start**:
1. Create audio dir and session dir.
2. **Move** (not copy) each chosen file into `<audio_dir>/`. `shutil.move` is
   destructive to the source. Confirm this is wanted; a TUI should say so
   clearly. A failed `transcribe` after the move leaves the audio in the new
   place.
3. Run transcription (section 4), streaming events.
4. Enable Open folder; enable Index if `transcript.md` exists; log the hint
   *"Run 'Index to Memory' after your recap pass."*

> **Known GUI flaw to fix, not copy:** `_run_pipeline` ignores the subprocess's
> exit code. It always ends with "Complete!" and "N file(s) transcribed",
> even when whisper failed on every file. The port should read per-file state
> and report "K of N succeeded, E failed".

**Resolve unknown speakers**: for each unknown stem, ask `player` and
`character` (combo of campaign PCs, blank = GM). Rows left blank are skipped.
Writes `{player: <p or null>, character: <normalized or null>, role: "Player"|"GM"}`
via the surgical append (3.5), then reloads the campaign and refreshes labels.
The GUI detects unknowns by searching the label string for `⚠`; the port should
use a typed `Resolved | Unknown` enum.

**Index to Memory**:
1. **Reload** the campaign from disk (the GM may have edited aliases or seeded
   `review_only` since selecting it).
2. If the campaign has no `review_only`, warn with three choices:
   *Cancel*, *Index anyway*, *Run report first* (the last only if a session
   exists).
3. Chunk (section 5) → `chunks_path`. Fail if the JSONL is missing or empty.
4. Ingest (section 6). Progress: 0.1 chunking, 0.5 ingesting, 1.0 done.

**Alias report**: run the chunker `--report`, log its output, write
`alias_candidates.md` into the session dir, open it with `xdg-open`. Status:
*"Alias report ready. Nothing was written to canon_aliases.json."*

**Open folder**: `xdg-open <session_dir>`. In a TUI, consider showing the file
list in a pane too.

### 7.4 Capabilities the GUI never exposed (cheap wins)

`--force`, `--post-process-only`, `--cleanup`, `--ignore-words`, `--device`,
`--check`, `info`/`query` against the Chroma store, `doctor`. A TUI has room for
all of them.

### 7.5 Suggested layout (yazi/rmpc-style)

```
┌ Campaigns ─────┬ Session / Files ─────────────┬ Preview ──────────────────┐
│ emberfall/main   │ Session: 13   → S13          │ transcript.md / report /  │
│ emberfall/side  │ 2-userone_0.flac → Vashti    │ log (tabbed)              │
│ ...            │ 6-gmhandle_0.mp3 → GM        │                           │
│                │ 3-newuser_0.flac → ⚠ Unknown │                           │
├────────────────┴──────────────────────────────┴───────────────────────────┤
│ ▓▓▓▓▓▓▓░░░░░ File 3 / 5: 3-newuser_0.flac                                 │
│ s start  i index  r resolve  o open  f files  q quit        (mouse: click)│
└───────────────────────────────────────────────────────────────────────────┘
```
Keys: `hjkl`/arrows, `Enter`, `s`, `i`, `r`, `o`, `f`, `?`, `q`. Click and scroll on
every pane. Inherit the terminal's ANSI palette instead of hard-coding colours
(the original reason for leaving the GTK theme).

---

## 8. Suggested Rust architecture

Treat as a starting point for the planning pass, not a decision.

- **Crates** (verify current versions and fitness): `ratatui`, `crossterm`,
  `tokio` (subprocess + async tasks), `serde` + `serde_json` with
  `preserve_order`, `anyhow`/`thiserror`, `regex`, `clap`, `directories` or
  hand-rolled XDG (the XDG rules in 3.1 are specific; hand-rolling may be
  closer to the original), `csv` for the log/state files.
- **Workspace split**:
  - `core` (no UI): config, campaign, paths, speaker resolution, filter/merge,
    chunker, report, JSON splice. Pure functions, fully testable.
  - `tui`: ratatui app, event loop.
  - `python/`: `chroma_memory.py` (+ `campaign.py` if option 6.1.1).
- **Concurrency**: UI thread owns state; pipeline jobs run as tokio tasks and
  send `PipelineEvent`s over an mpsc channel. Never block the draw loop.
  Redraw on events and ticks, not continuously.
- **Subprocess handling**: whisper-cli and ffmpeg are the long jobs. Support
  cancel (kill the child, mark the file `Error: cancelled`). The GUI has no cancel.
- **Logging**: also write the startup/runtime log to
  `$XDG_STATE_HOME/ttrpg-transcriber/` (override `GM_TRANSCRIBER_LOG`), as the
  GUI does, so a panicked TUI leaves a trace after the alternate screen closes.
- **Terminal hygiene**: restore the terminal on panic (panic hook) or the
  user's shell is left broken.

---

## 9. Verification plan

Behaviour parity is the hard part. Build **golden tests** against the Python
implementation before porting each stage.

1. **Fixture**: a small synthetic campaign (the repo ships
   `canon_aliases.example.json`, entirely invented data) plus 2 to 3 tiny audio
   tracks or, better, pre-made whisper JSON files so tests don't need a GPU.
   Do not commit real transcripts or real player names.
2. **Stage goldens** produced by running the original Python/bash once:
   - `merged_transcript.tsv`, `consolidated_transcript.tsv`, `transcript.md`
     from archived whisper JSON (`-p` mode needs no audio: ideal).
   - `s<nn>_chunks.jsonl` from a `transcript.md`.
   - `alias_candidates.md` from a `transcript.md`.
3. Rust output must match **byte for byte**, except documented tie-ordering.
4. **JSON splice**: property test that appended output parses and that every
   original byte outside the insertion is unchanged.
5. **Path rules**: table-driven tests for multi- vs single-vault ids, traversal
   rejection, zero-padding (`S07` vs `Session 7`).
6. **Cross-check** with `campaign.py paths` for several campaigns and sessions.
7. Run the alias report on the same `transcript.md` in both and diff; expect
   score differences unless Ratcliff/Obershelp is ported exactly.

---

## 10. Known quirks and open decisions

Things found while reading. Each needs a deliberate call, not a silent copy.

| # | Item | Where | Suggested call |
|---|---|---|---|
| 1 | `discord_names` key: docs say trailing `_M` stripped, code doesn't | 4.5, `campaign` JSON comment | Check a real Craig file; fix docs or code |
| 2 | GUI reports "Complete" even when whisper failed | 7.3 | Report per-file success from the state file |
| 3 | Audio files are **moved**, not copied | 7.3 | Keep, but confirm and say so in the UI |
| 4 | State/log CSVs are hand-quoted; commas in messages aren't escaped | 4.2 | Use a real CSV writer, stay readable by old script during transition |
| 5 | `"gm" in label` substring makes any label containing "gm" the GM | 5.2 | Keep for parity; consider a stricter match later |
| 6 | Unrecognised transcript lines are silently dropped by the chunker | 5.1 | Warn with a count |
| 7 | `chroma_memory.py` imports `campaign.py` | 6.1 | Option 2: self-contained helper |
| 8 | No cancel; a long whisper run looks frozen | 4.9 | Add cancel; investigate whisper-cli progress output |
| 9 | Similarity metric (Ratcliff/Obershelp vs others) | 5.6 | Port exactly or re-tune threshold |
| 10 | Python interpreter location for the helper | 6.3 | New `python` config key |
| 11 | `-mc 0` vs `-mc 64` prompt/hallucination trade-off | 4.4 | Expose as a setting |
| 12 | `has_<PC>` order and chunk shape tie the index to the JSONL | 5.5 | Don't change without a re-ingest plan |

Open questions for the planning pass:
- Own the move-and-transcribe orchestration in Rust entirely, or keep
  `transcribe.sh` as a subprocess for a first milestone? (Strangler approach:
  TUI first calling the existing scripts, then replace stage by stage. Lower
  risk, delivers a usable TUI earlier.)
- One binary or `core` as a library crate other tools (an MCP server is
  mentioned in `chroma_memory.py`'s docstring) could reuse?
- Windows/macOS support, or Linux-only as today? (Script assumes `stat -c%s`,
  GNU coreutils, `xdg-open`.)

---

## 11. Suggested milestone order

1. **Skeleton**: ratatui shell, campaign list from the existing config, path
   summary. Read-only. Proves config + discovery parity (`doctor` output matches).
2. **Strangler run**: Start button shells out to the **existing**
   `transcribe.sh`, streams `PROGRESS:` lines into a gauge and log. Already
   replaces the GUI for transcription.
3. **Speakers**: file list with resolution, unknown-speaker flow with the
   surgical JSON append (port 3.5 with golden tests).
4. **Index flow**: shell out to the existing chunker and `chroma_memory.py`;
   review-banner and confirm dialog.
5. **Replace stages in Rust, one at a time, each behind golden tests**: filter
   and merge (4.6 to 4.7), then chunker (5.1 to 5.5), then alias report (5.6),
   then whisper/ffmpeg orchestration (4.4).
6. **Cleanup**: make the Chroma helper self-contained (6.1.2), retire the shell
   scripts, add cancel, polish.
7. **Cutover**: archive the GTK GUI in the original repo.

Milestone 2 matters most: it gives a working, snappier front end quickly and
turns the rewrite into a series of small verified swaps instead of one large one.
