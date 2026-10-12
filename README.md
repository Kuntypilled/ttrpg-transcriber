# ttrpg-tui

A terminal program that turns a Craig Discord multitrack recording of a tabletop RPG
session into a searchable, canon-corrected transcript, and keeps a per-campaign
vector store of everything said at the table.

Everything runs on your machine. Whisper runs on the GPU through whisper.cpp,
embeddings run on the CPU through sentence-transformers, and the vector store is a
SQLite file on disk. No audio, transcript or embedding leaves it.

```text
SESSION TRANSCRIBER
Emberfall · daggerheart · emberfall
┌ Campaigns (2) ─────────┐┌ Session ────────────────────────────────┐┌  Paths  Log  Transcript ────────────────────────────────┐
│Emberfall               ││Session    [ 2 ]  → S02   n edit         ││[00:00:04] GM                                            │
│emberfall/main · Vashti_││Audio dir  …udio/emberfall/main/Session 2││The Cinder Road runs out at the gates of Marrow Hall.    │
│Lantern Row             ││Result     transcript.md exists ✓        ││                                                         │
│thornwake/lantern · Mara││                                         ││[00:00:07] Vashti_Corrow                                 │
│                        ││Files (6)   ⚠ 1 unknown speaker(s)       ││We take the road slowly. Bram, keep your hand off that   │
│                        ││✓ 1-gmuser.flac                          ││sword.                                                   │
│                        ││    → GM (Casey)                         ││                                                         │
│                        ││✓ 2-someuser.flac                        ││[00:00:08] Bram_Ollander                                 │
│                        ││    → Vashti_Corrow (Morgan)             ││I am not touching the sword. I am resting my hand near   │
│                        ││✓ 3-anotheruser.flac                     ││the sword. Near it.                                      │
│                        ││    → Bram_Ollander (Sam)                ││                                                         │
│                        ││✓ 6-newguest.flac                        ││[00:00:11] GM                                            │
│                        ││    → ⚠ unknown ("6-newguest")           ││Rain has been falling for three days, and the lanterns   │
└────────────────────────┘└─────────────────────────────────────────┘└ lines 1-27 of 77 ───────────────────────────────────────┘
████████████████████████████████████████████████████████Complete   0:04 ████████████████████████████████████████████████████████
6 of 6 transcribed. transcript.md is in the Transcript tab.
[f] Files  [r] Resolve  [s] Start  [x] Cancel  [i] Index  [p] Rebuild  [a] Report  [o] Open folder  [?] Help  [q] Quit
```

(Every name above is invented. The screen is shortened to fit.)

## Contents

- [What it does](#what-it-does)
- [Requirements](#requirements)
- [Install](#install)
- [First run](#first-run)
- [Using it](#using-it)
- [Keys](#keys)
- [Commands](#commands)
- [Configuration](#configuration)
- [The campaign file](#the-campaign-file)
- [Where files go](#where-files-go)
- [How it works](#how-it-works)
- [Design notes](#design-notes)
- [Try it without whisper or recordings](#try-it-without-whisper-or-recordings)
- [Development](#development)

## What it does

- **Campaigns.** A campaign is a folder in your vault with a `canon_aliases.json`. The
  program finds them by looking; there is no list to maintain.
- **Choosing recordings.** A file picker (folders, multi-select) for the Craig tracks.
  Starting asks first and says plainly that the files are moved into the session's audio
  folder, not copied.
- **Speakers.** Craig names each track after the Discord user. The program looks the name
  up in the campaign's `discord_names` and shows who is speaking on every track. Names it
  does not know are flagged, and a form (`r`) adds them to the file without reformatting
  it.
- **Transcription.** Each track is converted with ffmpeg and transcribed with whisper.cpp,
  seeded with the campaign's proper nouns. A progress bar moves through each track, the
  log shows what whisper says, and cancel works at any time.
- **Cleaning up.** Filler lines are dropped, repeated lines collapsed, and the "Thank you."
  that whisper invents on silence is removed by a stated rule (see
  [the stock-phrase rule](docs/differences.md#the-stock-phrase-rule)). The tracks are merged
  by timestamp into `transcript.md`.
- **Honest results.** "4 of 5 transcribed, 1 failed", with the failing track named. A
  retry skips tracks that already succeeded; one key transcribes them again; another
  rebuilds the transcript from the saved whisper output with no audio at all.
- **Index to Memory.** The transcript is cut into overlapping chunks, safe spelling
  corrections are applied, and the chunks are stored in a per-campaign ChromaDB with
  local embeddings. `ttrpg-tui query` searches it.
- **Alias report.** A read-only scan of a transcript for likely misheard names, to help
  fill in the campaign's `review_only` list before the first index.
- **Terminal and mouse.** Three panes, a Transcript tab to read the result in the app,
  zoom, and clickable buttons. The colours are your terminal's own palette.

## Requirements

Linux. Nothing assumes a particular distribution.

| For | You need |
|---|---|
| Building | Rust (Arch: `sudo pacman -S rust`, or [rustup](https://rustup.rs)) |
| Transcribing | `ffmpeg`, `whisper-cli` from [whisper.cpp](https://github.com/ggml-org/whisper.cpp), and a ggml model file |
| Index, `info` and `query` | Python 3 with `chromadb` and `sentence-transformers` |
| The Open folder button | `xdg-open` |

Transcribing needs nothing else: no Python, no `bash`, no `jq`. A terminal of at least
100 x 28 is comfortable.

## Install

### 1. Build the program

```sh
git clone https://github.com/Kuntypilled/ttrpg-transcriber
cd ttrpg-transcriber
cargo install --path crates/ttrpg-tui      # puts ttrpg-tui in ~/.cargo/bin
```

Make sure `~/.cargo/bin` is on your `PATH` (fish: `fish_add_path ~/.cargo/bin`). To run it
without installing, use `cargo run --release -p ttrpg-tui`.

### 2. ffmpeg

`ffmpeg` is spelled the same on every distribution: `sudo pacman -S ffmpeg`,
`sudo apt install ffmpeg`, `sudo dnf install ffmpeg`.

### 3. whisper.cpp

The program only ever runs the `whisper-cli` binary, so which GPU backend it was built
with is whisper.cpp's business. Vulkan, CUDA, ROCm, SYCL, Metal and plain CPU all behave
the same from here, only slower or faster.

**Arch, CachyOS, EndeavourOS:** install it. The GPU backend is whichever `ggml-*` package
you add.

```sh
sudo pacman -S ggml-vulkan whisper-cpp     # AMD, Intel, or anything with a Vulkan driver
# sudo pacman -S ggml-cuda whisper-cpp     # NVIDIA
# sudo pacman -S ggml-hip whisper-cpp      # AMD through ROCm (much larger)
# sudo pacman -S whisper-cpp               # no GPU: the CPU backend is built into ggml
```

**Elsewhere:** build it, in a folder outside this repository.

```sh
git clone https://github.com/ggml-org/whisper.cpp ~/src/whisper.cpp
cd ~/src/whisper.cpp
cmake -B build -DGGML_VULKAN=1             # swap the flag for your backend
cmake --build build -j"$(nproc)" --config Release
sudo install -Dm755 build/bin/whisper-cli /usr/local/bin/whisper-cli
```

You need a C++ toolchain, `cmake`, the Vulkan headers and `glslc` (the Vulkan shader
compiler; the build fails partway through without it, with an error that never mentions
it) and, at run time, a Vulkan driver (on AMD with Mesa, `vulkan-radeon`). In fish, write
`-j(nproc)`.

Check with `which whisper-cli`. If yours has another name or lives elsewhere, set
`whisper_bin` ([Configuration](#configuration)).

### 4. The model

```sh
mkdir -p ~/.local/share/whisper.cpp/models
wget -c -O ~/.local/share/whisper.cpp/models/ggml-large-v3-turbo.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin
```

About 1.5 GiB. Keep it elsewhere if you like and set `whisper_model`.

### 5. ChromaDB, only for Index

```sh
python3 -m pip install --user --break-system-packages chromadb sentence-transformers
```

`--break-system-packages` is needed where the system Python is marked externally managed
(PEP 668); it installs into your own `~/.local`. A virtual environment works too, because
the program runs whichever Python you name:

```sh
python3 -m venv ~/.venvs/ttrpg
~/.venvs/ttrpg/bin/pip install chromadb sentence-transformers
# then set  "python": "~/.venvs/ttrpg/bin/python"  in the config file
```

`sentence-transformers` depends on `torch`, and the default wheel bundles about 2.7 GB of
NVIDIA runtime that never loads on an AMD or Intel GPU or on the CPU. Install the CPU build
first and the resolver reuses it (check the index URL against pytorch.org for your Python):

```sh
pip install torch --index-url https://download.pytorch.org/whl/cpu
```

The first Index downloads the embedding model (`BAAI/bge-base-en-v1.5`, override with
`TTRPG_EMBED`) and takes a while.

### 6. Check

```sh
ttrpg-tui check
```

lists what is installed and what is missing, with the install command for the package
manager it finds. It creates nothing.

### 7. Desktop entry, optional

```sh
cp examples/ttrpg-tui.desktop.example ~/.local/share/applications/ttrpg-tui.desktop
$EDITOR ~/.local/share/applications/ttrpg-tui.desktop     # check the Exec path
update-desktop-database ~/.local/share/applications
```

The entry runs the program in a terminal (`Terminal=true`). `.desktop` files do not expand
`~` or `$HOME`, so `Exec` needs an absolute path to `ttrpg-tui`
(`/home/you/.cargo/bin/ttrpg-tui`) unless your session's `PATH` already has it. If your
launcher ignores `Terminal=true`, put the terminal in the command instead, for example
`Exec=alacritty -e /home/you/.cargo/bin/ttrpg-tui`.

A desktop-launched program does not inherit your shell's environment, so put your vault
and tool locations in the config file ([Configuration](#configuration)) rather than in
environment variables.

## First run

**Tell it where your vault is.** There is no default, deliberately: a wrong guess fails
quietly with an empty campaign list. Two layouts are supported.

- One vault per world, a folder holding `<world>/0.2_campaigns/<campaign>/...`. Campaigns
  are then addressed as `<world>/<campaign>`, so two worlds can each have a campaign called
  `main` without sharing audio or vector stores.
- One vault for everything.

```sh
mkdir -p ~/.config/ttrpg-transcriber
cp examples/config.example.json ~/.config/ttrpg-transcriber/config.json
$EDITOR ~/.config/ttrpg-transcriber/config.json     # set vaults_root, or vault
ttrpg-tui doctor
```

`doctor` prints every folder the program resolved, whether it exists, and where each value
came from (environment variable, config file or default). With nothing configured it
prints these instructions.

**Create a campaign.** A campaign is any folder under the vault's campaigns folder that
contains a `canon_aliases.json`:

```sh
mkdir -p "<vault>/0.2_campaigns/my_campaign"
cp examples/canon_aliases.example.json "<vault>/0.2_campaigns/my_campaign/canon_aliases.json"
$EDITOR "<vault>/0.2_campaigns/my_campaign/canon_aliases.json"
```

(`<vault>` is `<vaults_root>/<world>`, or your single vault.) Read the `_comment` fields in
that file first; [The campaign file](#the-campaign-file) explains the parts that matter.

## Using it

1. Pick a campaign. The session number is offered: the one after your last finished
   session, or the same one again if its run never produced a transcript. `n` edits it.
2. `f` opens the picker, starting in `~/Downloads`. `Space` ticks a file, `a` ticks all,
   `l` opens a folder, `h` goes up, `c` accepts. A Craig `.zip` has to be unpacked first.
3. Each track shows who is speaking. If any say "unknown", press `r` and fill in the
   player and character; the new entries are added to `canon_aliases.json` in the style of
   the entries already there.
4. `s` starts. The confirmation names the folder, says the files are moved, warns about
   same-name files it will replace, and shows the invented-lines rule (`t` keeps those
   lines for this run; `F` transcribes finished tracks again). Enter to go.
5. Watch the Log tab, or leave it. `x` cancels; the track in flight is retried next time.
   When it finishes, the Transcript tab opens on the result.
6. Correct `transcript.md` by hand, and add the misheard names you find to the campaign
   file. `p` rebuilds the transcript from the saved whisper output when you change a
   setting or resolve a speaker; it warns first that it rewrites `transcript.md`.
7. `a` writes `alias_candidates.md` (a report, nothing else) and opens it. Use it before
   the first index of a new campaign.
8. `i` indexes the transcript into the campaign's memory. A campaign with no `review_only`
   list is asked about first.

After a run, the audio is in the session's audio folder, so pressing `s` again with
nothing chosen retries: tracks that succeeded are skipped, failed ones are tried again.

## Keys

Everything has a key and a mouse equivalent: click a campaign, a tab, a button on the
bottom row, or the session number; the wheel moves or scrolls whatever is under the
pointer. When the terminal is too narrow for every button, the least used drop out first;
their keys still work.

| Key | Does |
|---|---|
| `Tab`, `h`, `l` | move between the three panes |
| `j` `k`, arrows, `g` `G`, `PgUp` `PgDn` | move or scroll in the focused pane (in the log, `g` and `G` go to oldest and newest) |
| `n` | type the session number (`+` and `-` step it) |
| `f` | choose audio files (in the picker: `Space` tick, `a` all, `l` open, `h` up, `.` hidden, `c` done) |
| `r` | resolve unknown speakers: add them to the campaign's `discord_names` (in the form: `Tab` next field, `Ctrl-N` `Ctrl-P` pick a character, `Ctrl-S` or `Enter` on the last field saves, `Esc` cancels) |
| `s` | start (asks first; at the question, `t` keeps or drops whisper's made-up lines for this run, and `F` transcribes the finished tracks again) |
| `p` | rebuild the transcripts from the saved whisper output (asks first; no audio or whisper needed) |
| `x` | cancel the running job |
| `i` | index the transcript into the campaign's memory (asks first if there is no `review_only` list) |
| `a` | alias report: likely garbled names, written to `alias_candidates.md` and opened |
| `o` | open the session folder |
| `c` | clear the chosen files |
| `[` `]` or `1` `2` `3` | Paths / Log / Transcript tab |
| `z` | zoom the right-hand pane to the full screen (`Esc` or `z` restores) |
| `R` | rescan campaigns and tools |
| `!` | what is installed and what is missing |
| `?` | help |
| `q` | quit (press twice while a job runs: it is cancelled first) |

## Commands

With no command, `ttrpg-tui` opens the program. The others print plain text and work in
scripts.

| Command | Does |
|---|---|
| `doctor` | every root the program resolved and where each value came from |
| `check` | are `ffmpeg`, `whisper-cli`, the model and ChromaDB installed? Creates nothing |
| `list` | slug, label and system of every campaign, tab-separated |
| `paths --campaign ID [--session N]` | the campaign's folders as shell-eval-able `KEY='value'` lines |
| `whisper-device` | the configured GPU index, or nothing |
| `info CAMPAIGN` | where the campaign's memory store is, how many chunks it holds, which sessions |
| `query CAMPAIGN "text"` | search the store: `--from SESSION`, `--char NAME`, `--contains TEXT`, `-k COUNT` |

`ID` is `<world>/<campaign>` with a vaults root, or the folder name with a single vault.
Exit codes: 2 for a problem you can fix (printed as `ERROR: ...` on stderr), 1 when
`check` finds a required tool missing or `doctor` finds no campaign.

## Configuration

Every setting resolves in the same order: environment variable, then
`$XDG_CONFIG_HOME/ttrpg-transcriber/config.json` (usually
`~/.config/ttrpg-transcriber/config.json`), then a built-in default. `doctor` shows which
one won. [`examples/config.example.json`](examples/config.example.json) lists every key.

| Setting | Environment variable | Config key | Default |
|---|---|---|---|
| vaults root (one vault per world) | `GM_VAULTS_ROOT` | `vaults_root` | none; wins over `vault` |
| vault (a single one) | `GM_VAULT`, `VAULT` | `vault` | none; one of the two is required |
| audio | `GM_AUDIO_ROOT` | `audio_root` | `$XDG_DATA_HOME/ttrpg_audio` |
| vector stores | `GM_CHROMA_ROOT` | `chroma_root` | `$XDG_DATA_HOME/ttrpg_memory` |
| whisper scratch files | `GM_WORK_ROOT` | `work_root` | `$XDG_CACHE_HOME/ttrpg_transcribe` |
| campaigns folder inside a vault | `GM_CAMPAIGNS_SUBDIR` | `campaigns_subdir` | `0.2_campaigns` |
| transcripts folder inside a campaign | `GM_TRANSCRIPTS_SUBDIR` | `transcripts_subdir` | `08_transcripts` |
| whisper command | `GM_WHISPER_BIN` | `whisper_bin` | `whisper-cli` on `PATH` |
| model file | `GM_WHISPER_MODEL` | `whisper_model` | `~/.local/share/whisper.cpp/models/ggml-large-v3-turbo.bin` |
| GPU index (`-dev`), for machines with more than one | `GM_WHISPER_DEVICE` | `whisper_device` | unset: whisper-cli's default |
| how much earlier text whisper sees (`-mc`; -1 is whisper's own default) | `GM_WHISPER_MC` | `whisper_max_context` | `0` |
| Python that has ChromaDB | `GM_PYTHON` | `python` | `python3` on `PATH` |
| phrases whisper makes up on silence | `GM_STOCK_PHRASES` (comma-separated, or `off`) | `stock_phrases` (a list, or `[]` for off) | `thank you`, `we'll be right back` |
| how long such a line must last to be dropped | `GM_STOCK_MIN_SECONDS` | `stock_min_seconds` | `20` |
| lines dropped as filler (the whole line, case ignored) | `GM_IGNORE_WORDS` (comma-separated, or `off`) | `ignore_words` (a list, or text) | `you`, `silence`, `um`, `uh`, `ah`, `like`, `right`, `well` |
| the config file itself | `GM_TRANSCRIBER_CONFIG` | | the path above |
| the store of one campaign, overriding the one above | `TTRPG_DB` | | `<chroma_root>/<campaign id>` |
| the embedding model | `TTRPG_EMBED` | | `BAAI/bge-base-en-v1.5` |

`~` is expanded in paths. The program's own log is `ttrpg_tui.log` under
`$XDG_STATE_HOME/ttrpg-transcriber/` (`GM_TRANSCRIBER_LOG` moves it).

## The campaign file

One `canon_aliases.json` per campaign, at
`<vault>/0.2_campaigns/<campaign>/canon_aliases.json`. It is the entire configuration and the
entire registry: there is no second file and no list to keep in sync. It is hand-maintained
and the program treats it that way: nothing rewrites it wholesale, `_comment` fields survive
every automated edit, and the resolve form adds entries by splicing text in.

```jsonc
{
  "campaign": "Emberfall",          // display label
  "world":    "emberfall",          // informational; two campaigns can share a world
  "system":   "daggerheart",        // shown in the campaign list

  // Fed to whisper.cpp --prompt. Biases the decoder toward canon spellings.
  "seed_prompt": "A Daggerheart tabletop session. Characters: Vashti, Nell, ...",

  // SAFE substitutions, applied automatically, case-insensitively, on word
  // boundaries, in file order. Every variant is a string that cannot mean
  // anything else in English.
  "aliases": {
    "Vashti": ["vashtee", "Vashty", "Vash Tea"],
    "Dunwell": ["dun well"]
  },

  // NEVER applied. Same shape, opposite contract (see Design notes).
  "review_only": {
    "Corrow":      ["borrow", "sorrow"],
    "Marrow Hall": ["narrow hall"]
  },

  // Canon proper nouns. Input to the alias report.
  "names": ["Vashti", "Nell", "Marrow Hall"],

  // Craig filename stem -> speaker. The stem is the file name without the
  // leading "N-" track number and without the extension.
  "discord_names": {
    "gmuser":      {"player": "Casey", "character": null,    "role": "GM"},
    "anotheruser": {"player": "Sam",   "character": "Bram",  "role": "Player"}
  },

  // Transcript speaker label -> player/character, used by the chunker. `match`
  // is compared case-insensitively as a SUBSTRING of the label and the first
  // match wins, so put specific entries first. character: null means the GM.
  "speaker_map": [
    {"match": "bram", "player": "Sam",   "character": "Bram"},
    {"match": "gm",   "player": "Casey", "character": null}
  ],

  // Character -> player. Key ORDER defines the order of the has_<PC> fields in
  // chunk metadata, so reordering these invalidates existing chunks.
  "players": {"Vashti": "Morgan", "Bram": "Sam", "Kestrel Vane": "Jordan"}
}
```

Every key is optional, except that you need `speaker_map` or `players` for the chunker to
attribute anyone. `{}` is a valid, useless campaign. Character names are normalised by
replacing whitespace with underscores, so `Kestrel Vane` in `players` and `Kestrel_Vane` in a
transcript label are the same character, and `has_Kestrel_Vane` stays a single metadata
token.

A real `canon_aliases.json` holds players' real names and Discord handles. It belongs in
your vault, not in a repository, and `.gitignore` keeps one from being committed here by
accident.

## Where files go

Resolved per campaign and session. `<id>` is `<world>/<campaign>` with a vaults root, or the
bare folder name with a single vault. Sessions below 10 are zero-padded: `S07`,
`s07_chunks.jsonl`.

```text
transcript    <vault>/0.2_campaigns/<c>/08_transcripts/S<nn>/transcript.md
merged        <vault>/0.2_campaigns/<c>/08_transcripts/S<nn>/merged_transcript.tsv
consolidated  <vault>/0.2_campaigns/<c>/08_transcripts/S<nn>/consolidated_transcript.tsv
whisper JSON  <vault>/0.2_campaigns/<c>/08_transcripts/S<nn>/archive/
run records   <vault>/0.2_campaigns/<c>/08_transcripts/S<nn>/transcription_log.csv, transcription_state.csv
alias report  <vault>/0.2_campaigns/<c>/08_transcripts/S<nn>/alias_candidates.md
chunks        <vault>/0.2_campaigns/<c>/08_transcripts/chunks/s<nn>_chunks.jsonl
audio         $GM_AUDIO_ROOT/<id>/Session <n>/
vector store  $GM_CHROMA_ROOT/<id>/
scratch       $GM_WORK_ROOT/<id>/S<nn>/temp/
```

Text that is cheap to keep and impossible to rebuild (the transcript, the chunk file, the
raw whisper output) lives in the vault. Binary or regenerable things (audio, the vector
store, scratch files) live outside it. Setting `GM_VAULT` to an empty scratch folder gives
you a full sandbox.

`ttrpg-tui paths --campaign ID --session N` prints all of these for one campaign.

## How it works

```text
Craig .flac tracks (one per speaker)
        |
        v
[1] transcribe     ffmpeg to 16 kHz mono WAV, then whisper-cli with -mc 0 and the
                   campaign's seed_prompt; the raw JSON is archived
    post-process   filler filter, duplicate collapse, invented-line rule,
                   speaker label from discord_names, merge by timestamp,
                   consolidate consecutive same-speaker lines
        |
        v
    transcript.md      <-- you read and correct this
        |
        v
[2] chunk          safe alias substitution, ~280-word windows with one utterance of overlap
        |
        v
    s<nn>_chunks.jsonl     <-- the durable artifact: text, lives in the vault
        |
        v
[3] store          chroma_memory.py (Python): upsert into a per-campaign persistent
                   collection, local BAAI/bge-base-en-v1.5 embeddings
        |
        v
    $GM_CHROMA_ROOT/<id>/      <-- regenerable, lives outside the vault
```

Stages 1 and 2 are Rust. Stage 3 stays Python because ChromaDB and sentence-transformers
have no practical Rust equivalent that reads an existing store. The helper
(`python/chroma_memory.py`) is built into the program and run with `python -c`, so there
is nothing to install beside the binary. It knows nothing about campaigns: the program
passes it the database folder and the player characters.

**`transcript.md`**

```markdown
**[00:41:30] Kestrel_Vane:**
Maybe it's survivor's guilt.

**[00:41:34] GM:**
Roll for it.
```

Timestamps are `HH:MM:SS` from the start of the recording. The two trailing spaces after the
colon are a Markdown hard line break, so it renders properly in Obsidian.

**Chunk JSONL**, one object per line:

```json
{"id": "s13_c000", "document": "[00:00:00] GM: Welcome back...\n[00:00:14] Bram: ...",
 "metadata": {"session": 13, "date": "2026-06-23", "start": "00:00:00", "end": "00:02:03",
              "has_gm": true, "n_utt": 8, "characters": "Kestrel_Vane,Bram,Nell,Vashti",
              "has_Vashti": true, "has_Nell": true, "has_Bram": true, "has_Kestrel_Vane": false}}
```

Chunk ids are deterministic (`s{session:02d}_c{index:03d}`) and storing is an upsert, so
re-running a corrected transcript overwrites in place instead of duplicating. The
`has_<PC>` booleans exist because Chroma's `where` filter cannot do substring matching on a
metadata string. The `date` is the day Index was run, not the session date.

## Design notes

**`aliases` and `review_only` are two lists and must never be merged.** `aliases` holds
manglings that cannot mean anything else (`Vashtee`) and are replaced automatically.
`review_only` holds manglings that are also real English words (`borrow` for Corrow, `vain`
for Vane, `narrow` for Marrow). Those are never replaced by any code path; they exist so a
human review pass knows where to look. Replacing "complete" with "Corrow" would corrupt every
legitimate use of the word in a 15,000-word transcript, quietly, and the damage would surface
months later as a retrieval query that returns nonsense. Fill in `review_only` before the
first index of a campaign; the alias report (`a`) does the scanning and splits every
candidate by whether it is an English word. It writes nothing to `canon_aliases.json`.

**One vector store per campaign, not one store with a campaign field.** Two campaigns can
share a world and players and still not share knowledge. A metadata filter is something you
can forget to apply; a separate store path cannot be forgotten. A corrupt index then costs
one campaign instead of all of them.

**If the embedding model changes, the store is invalid and the JSONL is not.** Never mix
models in one collection: the vector space changes and old vectors silently stop matching new
queries. Rebuild by indexing again.

**Speaker identity is explicit.** The program does not infer who is speaking from file name
heuristics beyond the documented Craig convention, and never from content. Craig's naming has
changed before; an explicit map fails loudly at a known place, where inference would keep
working and quietly attribute half a session to the wrong character.

**Edits to the campaign file are surgical.** Key order is meaningful, `_comment` fields
document intent, and entries may be column-aligned by hand, so a load-and-dump would destroy
all of that. New entries are spliced in as text, the result is parsed and checked to leave
every existing key as it was, and the file is replaced atomically. It refuses rather than
damages.

**Output follows the earlier tool's files.** The chunk file and the transcript formats are
the ones the earlier version of this tool wrote, so existing vector stores and transcripts
keep working. [`docs/differences.md`](docs/differences.md) lists every deliberate difference.

## Try it without whisper or recordings

```sh
scripts/demo.sh
```

builds a throwaway world (an invented campaign, six short "Craig" tracks in a Downloads
folder, one from a speaker the campaign does not know, and a stand-in `whisper-cli` that
takes a few seconds per track) and opens the program on it. Nothing outside the demo folder
is touched, and the vector store is pretended so ChromaDB is not needed. Follow the hints it
prints: `f`, `a`, `c`, `s`, Enter; afterwards `r` resolves the unknown guest.

Variations: `DEMO_FAIL=1` adds a track that fails, `DEMO_DELAY=0` makes runs instant,
`--keep` reuses last time's world so you can see skipped tracks. Anything after `--keep`
goes to the program, so `scripts/demo.sh --keep info emberfall/main` and
`scripts/demo.sh --keep query emberfall/main "the ledger"` work on the demo world.

## Development

| Path | What |
|---|---|
| `crates/ttrpg-core` | Library, no UI: settings, campaigns, paths, speaker resolution, the alias-file edit, the transcription run and its post-processing, the chunker, the alias report |
| `crates/ttrpg-tui` | The `ttrpg-tui` program: commands, and the app as a library so tests can drive it |
| `python/` | The Chroma helper and its requirements. See `python/README.md` |
| `examples/` | Invented example config, alias file and desktop entry |
| `docs/differences.md` | How this version differs from the earlier GTK, bash and Python one |
| `archive/` | The earlier version's files, kept for reference. Nothing here runs them |
| `scripts/demo.sh` | The demo above |

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Four kinds of test:

- **Golden files from the earlier scripts.** The post-processing, the chunker, the alias
  report and a whole run (`ffmpeg`, a stub `whisper-cli`, five tone files) are compared byte
  for byte with what the earlier scripts wrote on the same input. The `generate.sh` next to
  each set of files says how it was made.
- **Parity.** The same commands through `crates/ttrpg-core/tests/oracle/campaign.py` and the
  Rust code must print the same thing, and the alias-file edit must produce the same bytes as
  the original's `append_object_entries`.
- **End to end.** The real runner with real `ffmpeg` and a stub `whisper-cli`, driven through
  the job runner and through the whole app with keystrokes and mouse clicks, including
  cancel, retry, rebuild and every failure a run can report. Index runs the real Chroma
  helper against a stand-in for `chromadb`.
- **Screen.** Each pane, tab and dialog is drawn into an in-memory terminal and read back, at
  many sizes.

The parity tests need `python3`; the end-to-end ones need `bash` (for the stub), `ffmpeg` and
`python3`. Without them the tests print `SKIPPED` and pass. Real ChromaDB and the embedding
model are not tested.

Fixtures and examples contain only invented names. Real campaign files, transcripts, audio
and logs do not belong in this repository.
