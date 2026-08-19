# Session Transcriber

A four-stage pipeline that turns a Craig Discord multitrack recording of a tabletop RPG
session into a searchable, canon-corrected transcript and a per-campaign vector store.

Local-only. No audio, no transcript and no embedding ever leaves the machine. Whisper runs
on the GPU via whisper.cpp, embeddings run on CPU via sentence-transformers, and the vector
store is a SQLite file on disk.

Written for Linux. Developed on an AMD GPU with the whisper.cpp Vulkan backend under a
tiling compositor, but nothing here is tied to that: whisper-cli only has to be on PATH,
built with whatever backend your hardware wants.

It is not packaged and not on PyPI. It is a directory of scripts that find each other from
`__file__`, so the checkout can live anywhere. No path is compiled in. Every root resolves
from an environment variable, then a config file, then a default, and
`python3 campaign.py doctor` prints which of the three each one came from. See INSTALL.md.

---

## Contents

1. The problem
2. Architecture
3. Repository layout
4. Requirements
5. Configuration: `canon_aliases.json`
6. Usage
7. Path resolution
8. Data formats
9. Design decisions
10. The vault, and how the whole workflow fits together
11. What Claude does here, and what it does not
12. Adding a campaign
13. Known limitations

---

## 1. The problem

A session is four to five hours of five people talking over each other on Discord. Craig
(a Discord recording bot) hands back one audio file per speaker, which is the good case:
per-speaker tracks mean speaker attribution is a filename lookup rather than a diarization
problem.

Three things make this harder than "run Whisper on it":

**Proper nouns.** A campaign invents hundreds of names that no ASR model has ever seen.
Whisper will hear "Vashti" as Vashtee, Vashty, Vash Tea, Bash Tea, and Fashion,
sometimes several ways in the same minute. Uncorrected, the transcript is useless for
retrieval, because the thing you want to search for is spelled six ways.

**Some of those manglings are real English words.** Whisper hears "Corrow" as "borrow",
"Vane" as "vain", and "Marrow Hall" as "narrow hall". You cannot fix these
with a find-and-replace, because the transcript is also full of legitimate uses of
"complete" and "leave". Auto-replacing them destroys the transcript, and does it quietly,
which is worse.

**The transcript is evidence, not canon.** It records what was said at the table, including
things the GM later rules differently. It has to stay separate from the notes that record
what is actually true.

The pipeline addresses all three. The `aliases` / `review_only` split in the config is the
core idea, and section 9 explains it.

---

## 2. Architecture

```
  Craig .flac tracks (one per speaker)
            |
            v
  [1] transcribe.sh                       bash + whisper.cpp + ffmpeg + jq
      per-track ASR, seeded with the campaign's proper nouns
      filler filtering, duplicate collapse, speaker attribution
      merge by timestamp, consolidate consecutive same-speaker lines
            |
            v
      transcript.md            <-- human reads and corrects this
            |
            v
  [2] transcript_to_chunks.py             python, stdlib only
      safe alias substitution
      ~280-word conversational windowing with metadata
            |
            v
      s<nn>_chunks.jsonl       <-- the durable artifact; text, lives in the vault
            |
            v
  [3] chroma_memory.py                    python + chromadb + sentence-transformers
      upsert into a per-campaign persistent Chroma collection
      local BAAI/bge-base-en-v1.5 embeddings
            |
            v
      ~/.local/share/ttrpg_memory/<campaign>/    <-- regenerable, lives outside the vault

  [4] transcribe_gui.py                   PyGObject / GTK3
      drives 1 through 3, campaign selector, speaker resolution

      campaign.py                         the registry and path resolver
      imported by 2, 3 and 4; queried as a subprocess by 1
```

Stage 1 is bash because it is a pipeline of ffmpeg, whisper-cli, jq, sort and awk, and bash
is the right language for gluing those together. Stages 2 and 3 are Python because they do
regex work and talk to a vector DB. Stage 4 is GTK because it needs to be a desktop launcher
icon that a person clicks on a Sunday evening.

**`campaign.py` is the only place path rules exist.** Stage 1 does not reimplement them in
bash; it shells out to `python3 campaign.py paths --campaign X --session N` and `eval`s the
shell-quoted `KEY=value` output. One source of truth, at the cost of one subprocess per run.

---

## 3. Repository layout

```
campaign.py                    registry, path resolution, surgical JSON editing
transcribe.sh                  stage 1
transcript_to_chunks.py        stage 2, plus the alias-candidate report mode
chroma_memory.py               stage 3
transcribe_gui.py              stage 4
launch_transcriber.sh          wrapper called by the .desktop entry

canon_aliases.example.json     copy into a campaign folder in your vault
config.example.json            copy to ~/.config/ttrpg-transcriber/config.json
ttrpg-transcriber.desktop.example
requirements.txt               the two pip dependencies, and what is not pip-installable
INSTALL.md                     setup
README.md                      this file

layout-master.sh               niri window layout helpers, unrelated to the pipeline
layout-3x2-horizontal.sh
```

The checkout can live anywhere. Every file finds its siblings from `__file__` or
`BASH_SOURCE`, so there is no install step and no path to edit.

There is no real `canon_aliases.json` in this repository, and `.gitignore` keeps one from
being committed by accident. It holds players' real names and Discord handles, and
configuration lives per-campaign in the vault, which is the whole point of the
campaign-agnostic rework.

---

## 4. Requirements

System packages: `ffmpeg`, `jq`, and the GTK3 Python bindings (PyGObject). Package names
differ by distribution, so INSTALL.md lists them per family rather than assuming one.
`./transcribe.sh --check` detects the package manager on the running machine and prints the
install command for whatever is missing.

whisper.cpp with `whisper-cli` on `$PATH`. The Vulkan backend is what this was developed
against; CUDA, ROCm, Metal and plain CPU all work the same as far as this tool is concerned,
because it only ever shells out to the binary.

```
git clone https://github.com/ggml-org/whisper.cpp
cd whisper.cpp
cmake -B build -DGGML_VULKAN=1
cmake --build build -j$(nproc) --config Release
sudo install -Dm755 build/bin/whisper-cli /usr/local/bin/whisper-cli
```

On Arch and derivatives, check the AUR before building: a `whisper.cpp-vulkan` package
exists. Read the PKGBUILD first, as with anything from the AUR.

The model, roughly 1.5 GB:

```
mkdir -p ~/.local/share/whisper.cpp/models
wget -c -O ~/.local/share/whisper.cpp/models/ggml-large-v3-turbo.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin
```

Python packages for stage 3 only. Stages 1, 2 and 4 need nothing beyond the stdlib and
PyGObject:

```
pip install --user --break-system-packages chromadb sentence-transformers
```

Verify the lot:

```bash
./transcribe.sh --campaign emberfall --check
```

---

## 5. Configuration: `canon_aliases.json`

One file per campaign, at `$VAULT/0.2_campaigns/<campaign>/canon_aliases.json`. It is the
entire configuration surface and the entire campaign registry. There is no second config
file, no registry list, nothing to keep in sync.

It is hand-maintained, and the tool treats it as such: nothing rewrites it wholesale, and
the `_comment` fields are load-bearing documentation that survive every automated edit.

```jsonc
{
  "_comment": "...",
  "campaign": "emberfall",          // display label
  "world":    "emberfall",          // informational; two campaigns can share a world
  "system":   "daggerheart",          // shown in the GUI selector

  // Fed to whisper.cpp --prompt. Biases the decoder toward canon spellings.
  "seed_prompt": "A Daggerheart tabletop session. Characters: Vashti, Nell, ...",

  // SAFE substitutions. Applied automatically, case-insensitively, on word
  // boundaries, in file order. Every variant here is a string that cannot mean
  // anything else in English.
  "aliases": {
    "Vashti": ["vashtee", "Vashty", "Vashtie", "Strove Scott"],
    "Wren":     ["Ren"],
    "Dimwell":  ["dim well"]
  },

  // NEVER applied. Same shape, opposite contract. See section 9.
  "review_only": {
    "_comment": "Manglings that collide with real English words.",
    "Corrow":      ["borrow", "sorrow"],
    "Vane":        ["vain", "vein"],
    "Marrow Hall": ["narrow hall"]
  },

  // Canon proper nouns. Input to the alias-candidate report.
  "names": ["Vashti", "Nell", "Thornwake", "Marrow Hall", ...],

  "pc_nicknames": { "Nell": ["Vash", "Little Vash"] },

  // Craig filename stem -> speaker. The stem is the filename minus the leading
  // "N-" track number and the extension. Written by the GUI's resolve dialog.
  "discord_names": {
    "gmuser":        {"player": "Casey",  "character": null,           "role": "GM"},
    "anotheruser": {"player": "Sam",  "character": "Bram",        "role": "Player"}
  },

  // Transcript speaker label -> player/character. `match` is compared
  // case-insensitively as a SUBSTRING of the label, and ORDER MATTERS: first
  // match wins. character: null means the GM.
  "speaker_map": [
    {"match": "bram",    "player": "Sam",  "character": "Bram"},
    {"match": "vashti", "player": "Alex",    "character": "Vashti"},
    {"match": "gm",       "player": "Casey",  "character": null}
  ],

  // Character -> player. Key ORDER defines the order of the has_<PC> fields in
  // chunk metadata, so reordering these invalidates existing chunks.
  "players": {
    "Vashti": "Alex", "Nell": "Riley", "Bram": "Sam", "Kestrel Vane": "Jordan"
  }
}
```

Every key is optional except that you need `speaker_map` or `players` for stage 2 to resolve
anyone. A file containing `{}` is a valid, useless campaign.

Character names are normalised by replacing whitespace with underscores, so `Kestrel Vane`
in `players` and `Kestrel_Vane` in a transcript speaker label are the same character, and
`has_Dash_Montoya` stays a single metadata token.

---

## 6. Usage

### GUI

```bash
./launch_transcriber.sh
```

Pick a campaign from the dropdown, type a session number, choose the Craig audio files,
press Start. The file list resolves each track to a character as you add it; unrecognised
Discord usernames get flagged with a warning and a **Resolve Speakers** dialog that writes
them into the selected campaign's `discord_names`.

After the recap pass, **Index to Memory** runs stages 2 and 3.

### CLI

Stage 1, transcription:

```bash
./transcribe.sh --campaign emberfall --session 17
./transcribe.sh --campaign emberfall --session 17 --post-process-only
./transcribe.sh -i /path/to/audio --aliases /path/to/canon_aliases.json
```

`--post-process-only` replays the archived Whisper JSON through the formatting pipeline
without re-running ASR. It needs no audio, which matters because the audio gets deleted and
the archive does not.

Stage 2, chunking:

```bash
python3 transcript_to_chunks.py --campaign emberfall --session 17
python3 transcript_to_chunks.py --campaign emberfall --session 17 -o -    # stdout
```

Stage 2, alias-candidate report (read-only, writes nothing to `canon_aliases.json`):

```bash
python3 transcript_to_chunks.py --campaign second_campaign --session 1 --report
```

Stage 3, indexing and retrieval:

```bash
python3 chroma_memory.py ingest --campaign emberfall --session 17
python3 chroma_memory.py query  --campaign emberfall "the stress test reveal" --from 13
python3 chroma_memory.py query  --campaign emberfall "what the Warden said" --char Vashti
python3 chroma_memory.py info   --campaign emberfall
```

Registry introspection:

```bash
python3 campaign.py list
python3 campaign.py paths --campaign emberfall --session 17
```

---

## 7. Path resolution

| Variable | Default | Holds |
|---|---|---|
| `GM_VAULT` (or `VAULT`) | none, and unset is an error | the Obsidian vault |
| `GM_AUDIO_ROOT` | `~/Documents/TTRPG Session Audio` | Craig recordings |
| `GM_CHROMA_ROOT` | `~/.local/share/ttrpg_memory` | vector stores |
| `GM_WORK_ROOT` | `~/.cache/ttrpg_transcribe` | Whisper intermediates |
| `TTRPG_DB` | unset | overrides the resolved DB path |
| `TTRPG_EMBED` | `BAAI/bge-base-en-v1.5` | embedding model |

Resolved per campaign and session:

```
transcript   $VAULT/0.2_campaigns/<c>/08_transcripts/S<nn>/transcript.md
archive      $VAULT/0.2_campaigns/<c>/08_transcripts/S<nn>/archive/
chunks       $VAULT/0.2_campaigns/<c>/08_transcripts/chunks/s<nn>_chunks.jsonl
audio        $GM_AUDIO_ROOT/<c>/Session <n>/
vector store $GM_CHROMA_ROOT/<c>/
working      $GM_WORK_ROOT/<c>/S<nn>/temp/
```

Session numbers are zero-padded below 10: `S07`, `s07_chunks.jsonl`.

Setting `GM_VAULT` to a scratch directory gives you a full sandbox, which is how the test
runs for this tool are done without touching live session data.

---

## 8. Data formats

### `transcript.md`

```markdown
**[00:41:30] Kestrel_Vane:**
Maybe it's survivor's guilt.

**[00:41:34] GM:**
Roll for it.
```

Timestamps are `HH:MM:SS` from the start of the recording. The two trailing spaces are a
markdown hard line break, so it renders correctly in Obsidian.

### Chunk JSONL

One JSON object per line.

```json
{
  "id": "s13_c000",
  "document": "[00:00:00] GM: Welcome back...\n[00:00:14] Bram: ...",
  "metadata": {
    "session": 13,
    "date": "2026-06-23",
    "start": "00:00:00",
    "end": "00:02:03",
    "has_gm": true,
    "n_utt": 8,
    "characters": "Kestrel_Vane,Bram,Nell,Vashti",
    "has_Vashti": true,
    "has_Nell": true,
    "has_Bram": true,
    "has_Kestrel_Vane": false
  }
}
```

Windowing accumulates whole utterances until it passes 280 words, then starts the next
window one utterance back. Overlap of one utterance keeps a question and its answer from
being split across a boundary with no shared context.

Chunk IDs are deterministic (`s{session:02d}_c{index:03d}`) and ingestion is an upsert, so
re-running a corrected transcript overwrites in place instead of duplicating. This is the
property that makes the whole "correct the transcript, then re-index" loop safe to repeat.

The `has_<PC>` booleans are denormalised out of `characters` because Chroma's `where` filter
cannot do substring matching on a metadata string.

---

## 9. Design decisions

### `aliases` and `review_only` are two lists, and must never be merged

This is the load-bearing idea.

`aliases` holds manglings that cannot mean anything else: `Vashtee`, `Carolcroft`,
`Renfree`. These are auto-replaced on word boundaries at stage 2.

`review_only` holds manglings that are also real English words: `borrow` for Corrow,
`vain` for Vane, `narrow` for Marrow, `stable` for Sable, `nail` for Nell. These are
**never** substituted by any code path. They exist so that a human review pass knows where
to look.

The failure mode being avoided is specific and nasty. Auto-replacing "complete" with
"Corrow" corrupts every legitimate use of the word in a 15,000-word transcript, produces
output that still looks like a transcript, and is discovered months later when a retrieval
query returns nonsense. It is a silent, unbounded, retroactive data corruption bug. The two
lists exist so that no future edit can accidentally introduce it.

Consequently: `review_only` is worth populating **before** the first ingest of a new
campaign, not after. Stage 2 has a `--report` mode for exactly this. It scans a transcript
and produces a markdown report in two sections:

- near-misses of names already in canon, scored by `difflib` ratio
- capitalised tokens that are neither canon nor in `/usr/share/dict/words`

and it splits every candidate by whether it is a real English word, which is the
`aliases` / `review_only` decision made mechanically. It writes nothing to
`canon_aliases.json`. A human still makes the call. The GUI shows a standing banner for any
campaign with an empty `review_only` and asks for confirmation before that campaign's first
index.

### The campaign registry is the filesystem

A campaign is any directory under `0.2_campaigns/` containing a `canon_aliases.json`. That
is the entire registry.

The alternative, a `campaigns.json` listing them, is a second source of truth that will
drift from the directory structure the first time someone renames a folder. Globbing cannot
drift. Adding a campaign is `mkdir` plus one file, with no code change anywhere, which is
the headline acceptance criterion this rework was built against.

### One vector database per campaign, not one database with a `campaign` field

Two campaigns can share a world and share players and still not share knowledge: their
timelines run concurrently, and what one table has learned the other has not. A retrieval
query about that shared world, run for one table, must not surface the other table's
transcript.

That could be a metadata filter. A metadata filter is a thing you can forget to apply, and
one forgotten `where` clause spoils a mystery that took months to set up. A separate
`PersistentClient` path cannot be forgotten. It also means a corrupt HNSW index costs one
campaign instead of all of them.

The cost is that cross-campaign queries are impossible without opening both stores. That is
the correct default here.

### What lives in the vault and what does not

The vault is an Obsidian vault synced across devices.

**In the vault:** `transcript.md`, the chunk JSONL, the Whisper `archive/` JSON. All text,
all diffable, all either irreplaceable or cheap.

**Outside the vault:** audio, the Chroma store, Whisper working files.

The Chroma store in particular is a binary SQLite file plus an HNSW index. Syncing it means
pushing megabytes on every ingest, and two devices writing it concurrently has no locking
and can corrupt it. It is also fully regenerable from the JSONL in about a minute. So the
generated binary artifact lives in `~/.local/share/` and the text it was generated from
lives in the vault, which is the right way round.

One consequence worth writing down: **if the embedding model changes, the database is
invalid and the JSONL is not.** Never mix embedding models in one collection. The vector
space changes and old vectors silently stop matching new queries.

### Speaker identity is explicit, never inferred

`speaker_map` is a hand-written ordered list of substring matches. The tool does not attempt
to infer who is speaking from filename heuristics beyond the documented Craig convention,
and does not guess from the content.

Craig's filename convention has changed before. When it changes again, an explicit map fails
loudly at a known place. Inference would keep working and quietly attribute half a session
to the wrong character, and nothing downstream would notice.

### Config edits are surgical

`canon_aliases.json` is hand-maintained: key order is meaningful, `_comment` fields document
intent inline, and `discord_names` entries are column-aligned by hand.
`json.load` + `json.dump` would destroy all of that on the first automated write.

So `campaign.append_object_entries()` splices new entries in as text: locate the target
object with a brace matcher that respects string literals, add a comma to the previous
entry, insert the new lines, then parse the result and assert that every pre-existing key
still has its original value before writing. It refuses rather than damages.

### The refactor was verified byte-for-byte

This started as a single-campaign tool with the campaign identity hardcoded across five
files. The rework was held to a specific bar: given the same transcript and the same alias
file, the new code must produce **byte-identical** chunk JSONL to the old code.

That was verified across all four existing sessions, for both stage 1 and stage 2, by
running the pre-refactor scripts side by side with the new ones and `cmp`-ing the output.
It is the difference between a refactor and a rewrite, and it is the reason the existing
175-chunk vector store did not need to be rebuilt. The store was migrated by moving the
directory, because `chromadb.PersistentClient` does not care where it lives.

### GTK note

The campaign selector is a `MenuButton` with a `Popover`, not a `ComboBoxText`.

`GtkComboBox` pops a `GtkMenu`, and `GtkMenuShell` decides between "click to open, click to
pick" and "press, drag, release to pick" by comparing the button-release timestamp to when
the menu opened. Under XWayland on niri that heuristic misfires and every release dismisses
the menu, so the control only worked as a press-drag-release gesture. A `MenuButton` is a
toggle button and involves no timestamp heuristic.

---

## 10. The vault, and how the whole workflow fits together

The transcription tool is one component of a larger system. It is not the interesting part;
it is the part that feeds the interesting part.

### The organising rule

```
The world holds what is true whether or not anyone plays.
The campaign holds what happened at the table and what is planned next.
```

Thornwake the city is world. Thornwake as the party left it is campaign. An NPC's biography is
world. Whether this table has met her, and what they think of her, is campaign.

The test: would a second table running in this world need this note? A world with two
tables in it is where that test stops being hypothetical.

### Structure

```
gm_worldbuilding/
  0.0_inbox/            capture point, should empty most days
  0.1_worlds/           one folder per world
  0.2_campaigns/        one folder per campaign, each with a canon_aliases.json
  0.3_rules_reference/  three SRDs, never edited
  0.4_source_material/  published adventures
  0.5_toolkit/          templates, Bases, procedures, generators, house rules, style
  0.6_assets/           images, maps, handouts, tokens
  _archive/             superseded material
```

Inside a world: `01_regions` `02_locations` `03_factions` `04_npcs` `05_cultures`
`06_religion` `07_things` `08_history` `09_lore`

Inside a campaign: `01_sessions` `02_threads` `03_party` `04_arcs` `05_npc_state`
`06_prep_kit` `07_canon` `08_transcripts`

Every note carries YAML frontmatter with at least `type`, `world`, `campaign` and `status`.
This is not decoration. Obsidian's Bases plugin builds live views by querying frontmatter,
so a note missing `type` is invisible to every view, which is functionally the same as not
existing. The views are the actual interface to the vault: `Open Threads`, `Unpaid Setups`,
`NPC Roster`, `Where We Left Off`, `Table Knowledge`, `Needs Writing`, `Untyped Notes`.

Think of it as a document store with a hand-maintained schema and materialised views, where
the query engine is a markdown plugin and the write path is a person typing.

### The loop, end to end

**1. Conception.** A world starts as a premise and a handful of notes in `0.1_worlds/`.
Regions, factions, a religion, some history. Marked `status: seed` or `status: stub` until
they have content. `stub` is used deliberately: an empty note marked `active` is a lie the
vault will tell you six months later.

**2. Campaign setup.** A campaign folder gets a Campaign Truth note, a party roster in
`03_party/`, opening threads in `02_threads/`, and a `canon_aliases.json` seeded with the
proper nouns that exist so far. That last file is the handshake between the vault and the
transcription tool.

**3. Prep.** Per session, `01_sessions/S<nn>/S<nn> - Prep.md`, written against the Bases
views. `Open Threads` says what is live. `Unpaid Setups` says what has been foreshadowed and
not yet paid off. Prep is a GM-facing document: plain, scannable, read at speed at the table.

**4. Run.** The session is played. Craig records it. A run log gets written during or
straight after, beat by beat, fast and ugly, including anything that contradicted prep.

**5. Transcription.** This tool. Craig audio in, `transcript.md` out, seeded with the
campaign's canon so the proper nouns come back mostly right.

**6. Recap and capture.** The transcript is read against the run log, and the
`Post-session capture` procedure runs. This is the step that keeps the vault from collapsing
back into one document. The rule underneath it:

```
A session note records what happened once.
Anything with a future belongs in a note of its own.
```

So threads get their `02_threads/` notes updated with a `last_touched` and a line on where
they stand. NPCs whose relationship to the party moved get their `05_npc_state/` note
updated, with a `confidence` value, per table. Anything that contradicted prep gets a note
in `07_canon/` recording what prep said, what actually happened, and which one wins. New
places and factions get neutral-voice notes in `0.1_worlds/`.

Then the player-facing recap gets written. That is the one place in the vault the creative
voice profile applies in full.

**7. Indexing.** The corrected transcript is chunked and upserted into that campaign's
vector store. Now every session ever played is semantically searchable, filtered by session
range and by which characters were present.

**8. Back to 3.** Next session's prep queries the views, and the vector store when the views
are not enough. "What did the Warden actually say before she died" is a retrieval query against
raw transcript. "What is true about the Warden" is a vault note. The two layers never merge, and
on conflict the vault wins, because the transcript records what was said and the vault
records what was ruled.

### The feedback loop worth noticing

Step 6 produces canon corrections. Some of those corrections are transcription errors, and
they get written back into `canon_aliases.json`. That file is fed to `whisper.cpp --prompt`
on the next run, which biases the decoder toward the right spellings, and to stage 2's
substitution pass, which fixes the ones that still slip through.

So the tool's accuracy on session N+1 is a function of the corrections made in session N.
There is a real note in the vault, `07_canon/Seeded, not seated.md`, that reads in part:

> The Cycle 90 log title. Whisper transcribed "seated leadership." The packet reads
> "On seeded leadership." Logged to `review_only` in `canon_aliases.json`, since both are
> real words and it will never auto-replace.

That is the loop closing: an ASR error becomes a canon ruling becomes a config entry becomes
a permanent guard against the same error being silently auto-corrected the wrong way.

---

## 11. What Claude does here, and what it does not

Claude is used against this vault, with a `CLAUDE.md` at the vault root that carries the
structure, naming rules and writing conventions. A few purpose-built skills exist for the
repetitive parts: turning a transcript into a filled-out post-session capture, rendering
session pages as styled HTML for players, running the daily note.

The division of labour is deliberate, and it is the opposite of the usual pitch.

**What it is for:** the mechanical, high-volume, low-judgement work. Reading a 15,000-word
transcript against a run log and proposing which threads moved. Catching that an NPC's
status changed in session 14 and their `05_npc_state/` note still says otherwise. Finding
the twenty-three dead wikilinks caused by curly versus straight apostrophes. Refactoring
five hardcoded scripts into a campaign-agnostic pipeline and proving byte-for-byte that
nothing changed. Formatting, cross-referencing, consistency checking, and saying "prep said
X, the transcript says Y, which wins."

**What it is not for:** deciding what is true. The world, the campaign, the NPCs, the plot,
the rulings and the voice are the GM's. There is no "generate me a world" step in this
workflow and there is not going to be one. A world produced that way would be one nobody had
thought about, and the whole value of the vault is that somebody thought about all of it.

The vault encodes this. `Never invent a rule, a citation, or established canon.` Filling a
gap is allowed and must be marked as a suggestion. Everything produced is a draft for review.
On canon conflicts the newer human source wins and the older reading gets recorded rather
than deleted. The SRDs are read-only. The creative voice profile applies to exactly one
document type and nowhere else.

The useful frame: this is a system of record with a person as the author and a language
model as the librarian, the proofreader, and occasionally the build engineer. The librarian
does not get to write the books.

---

## 12. Adding a campaign

Two steps, no code:

```bash
mkdir -p ~/gm_worldbuilding/0.2_campaigns/<slug>/08_transcripts/chunks
$EDITOR ~/gm_worldbuilding/0.2_campaigns/<slug>/canon_aliases.json
```

Minimum viable config:

```json
{
  "campaign": "my-campaign",
  "system": "daggerheart",
  "seed_prompt": "A Daggerheart tabletop session. Characters: ...",
  "aliases": {},
  "review_only": {},
  "names": ["..."],
  "discord_names": {},
  "speaker_map": [
    {"match": "somename", "player": "Someone", "character": "Some Character"},
    {"match": "gm", "player": "Casey", "character": null}
  ],
  "players": {"Some Character": "Someone"}
}
```

Confirm it registered:

```bash
python3 campaign.py list
```

It now appears in the GUI dropdown with its own audio folder, transcript folder, chunk
folder and vector store. After the first session, run `--report` before the first index to
seed `review_only`.

---

## 13. Known limitations

- **Speaker attribution depends entirely on Craig's per-track output.** No diarization. If
  two people share a mic, they share a speaker label.

- **`speaker_map` matching is substring-based and order-dependent.** A `match` value that is
  a substring of another character's name will shadow it. Put specific entries first.

- **The alias-candidate report is noisy on short names.** `Wren` is four characters, so
  `were`, `when` and `went` all score above the default 0.70 threshold. They are correctly
  binned as review-only, where noise is harmless because nothing is auto-applied. Raise
  `--threshold` to quiet it.

- **The `--report` English-word test uses `/usr/share/dict/words`.** No dictionary means
  every candidate is classified as safe-to-alias, which is the wrong direction to fail. The
  `dictionaries-common` package is effectively a requirement of report mode.

- **`window()` accumulates whole utterances until it exceeds 280 words**, so a single very
  long GM monologue produces one oversized chunk. It has not been a retrieval problem in
  practice.

- **Re-chunking an old session with a newer alias set changes its output**, because more
  substitutions apply and word counts shift window boundaries. Chunk IDs are stable, so the
  upsert is still correct, but the JSONL is not reproducible across config revisions. The
  config is the input, not a constant.

- **Stage 3 imports `chromadb` and `sentence-transformers` at call time**, roughly a
  five-second cold start per invocation. Fine for a batch tool run once a week, wrong for
  anything interactive.

- **The GUI runs the pipeline in a `threading.Thread` and marshals UI updates through
  `GLib.idle_add`.** It is a subprocess driver, not a concurrent application, and it will
  not survive being asked to do two things at once.
