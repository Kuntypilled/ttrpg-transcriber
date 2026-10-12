# Errata to PORTING.md

PORTING.md is kept as it was handed over. This file records where the original
code at `98aa387` differs from it, found while porting milestone 1 and reading the
rest of the source, and the places where the Rust port deliberately differs from
the original.

Each finding says how it was established:

- **ran**: executed against invented data, with a stub `whisper-cli` that writes
  canned JSON (the real one was not available).
- **read**: read from the code only.
- **real data**: confirmed against a real Craig session log (2026-08-03) and a real
  `canon_aliases.json`, both supplied for reference. Neither is stored here.

## Differences between PORTING.md and the code

| Section | PORTING.md says | The code does |
|---|---|---|
| 4.4 step 1 | Skip a file with a `Success` row, "whole-line match on the first CSV field" | Splits the row on commas; the first field must be the file name and the rest of the row must contain `"Success"` as a substring. State rows are only appended, so one old Success row wins forever: after a failed `--force` rerun the file holds Success and Error rows for the same tracks, and the next normal run skips them all. The summary then reports 8 total, 4 success, 4 errors. (**ran**) |
| 4.6 | Skip empty lines | Skips a row only if start, end and text are all empty. A whitespace-only segment becomes a row with empty text. (**ran**) |
| 4.7 | "stable-sorted" and "don't promise a tie order" | `sort -k2,2n` is not stable. Ties are broken by comparing the whole line under the current locale. The C locale gave speaker-name byte order; a UTF-8 locale will likely differ. (**ran**) |
| 5.5 | Keep the chunk JSONL "byte-compatible" | Chroma parses each line, so only the parsed values matter for the index. Byte equality matters only for golden tests. Python's `json.dumps` writes `", "` and `": "` separators and escapes all non-ASCII (`It’s` becomes `It’s`); `serde_json` does neither by default, so a formatter that copies Python's output is needed. (**ran**) |
| 6.1 | `--db` "already supported", so only `--pcs` is missing | `--campaign` is required and `campaign.load()` always runs. `query --char` and `info` also use the campaign. Option 2 is a bigger edit than adding `--pcs`. (**read**) |
| 7.3 | Start, Index and the session field are re-enabled in a `finally` | Start and the session field are. Index is re-enabled on error paths but not after a successful index, so it stays disabled until the session or campaign changes. (**read**) |
| 10 #4 | State and log CSVs are "hand-quoted" | The state CSV is quoted without escaping embedded quotes. The log is not quoted at all, and whisper-cli's stderr is redirected into the same file: in the real log, 265 of 303 non-blank lines are whisper output, not log rows. (**real data**) |

## Bugs in the original that PORTING.md section 10 does not list

1. **`-p` replay re-strips the extension.** `get_player_name` is given a name that
   already lost its extension and strips from the last dot again. A username
   containing a dot is labelled correctly on a fresh run and wrongly on a replay
   (`2-some.user_0.flac`: fresh `Character_Name`, replay `2-some`). Section 9 proposes
   `-p` as the golden generator, so goldens would encode this. The five usernames in
   the real log contain no dots. (**ran**)
2. **`discord_names` key rule.** The code is right: the key is everything after the
   first `-`, minus the extension, matched exactly. Craig does not add a trailing
   `_M`, and the real alias file uses bare usernames. The comment in
   `canon_aliases.example.json` was wrong; `examples/canon_aliases.example.json` has
   it corrected, and nothing else in the file changed. (**real data**, **ran**)
3. **`--check` creates files.** It runs after the `mkdir`s and CSV creation. In an
   empty directory it created `transcriptions/archive`, `transcriptions/temp` and
   both CSVs. With `--campaign` and `--session` it would create the session folder
   in the vault. Do not call it at TUI startup. (**ran**; campaign mode **read**)
4. **Empty-text rows break the chunker.** When an empty-text row is its own entry in
   `transcript.md`, the chunker's regex takes the next speaker's header and text as
   its body. Whether real whisper.cpp output contains whitespace-only segments is
   not known. (**ran**)
5. **Unmatched speakers are labelled `GM:` in chunk text** while their role stays
   Player. (**ran**)
6. **The GUI and the script resolve speakers differently.** An entry with
   `role: "Player"` and a null `character` shows as "Player (name)" in the GUI and
   falls back to the file name in the script. The port needs one function. (**read**)
7. **`shutil.move` overwrites silently** when a same-name file is already in the
   session audio folder. (**ran**) Re-running over files already there works, because
   moving a file onto itself is a no-op. (**ran**) After a run the GUI keeps the old
   source paths selected, so pressing Start again fails at the move until the files
   are chosen again from the audio folder. (**read**)
8. **Chunk `date` is the day Index was clicked**, not the session date. (**read**)
9. **Alias report fidelity.** The dictionary comes from `/usr/share/dict/words` on the
   generating machine, so its golden needs a pinned wordlist in the fixture. Ordering
   also depends on Python's insertion-ordered dicts in `Counter.most_common`, the
   strict `>` in best-match selection, and stable sorts. (**read**)
10. **Settings that exist only as command-line flags**: model path, whisper binary,
    ignore words, and `-mc`. The TUI needs config keys for them. (**read**)

## Observations from the real session log

Five tracks of about 73 minutes (4376 to 4387 s) transcribed in 19 to 38 s each, and
the whole session, from the first line to the last, took 2 min 11 s. A long track
looks frozen for under 40 s, so cancel is worth having and in-file progress is low
priority.

## Where the Rust port deliberately differs from `campaign.py`

| Difference | Why |
|---|---|
| `menu_label` is not ported; the UI shows the label and the system separately | The original's separator is an em dash |
| Messages say `ttrpg-tui ...` instead of `python3 campaign.py ...`, and the example campaign id is `emberfall/main` | This is a different program |
| The text of a JSON syntax error is `serde_json`'s | Python's wording cannot be reproduced; the file is still named |
| A config or alias value of the wrong JSON type where a path or string is expected (array, object), or an alias file that is not an object, is an error naming the file and key | The original crashed or built a nonsense path |
| A config file that exists but cannot be read is an error naming it | The original crashed |
| Root paths are used literally when scanning for campaigns, not as glob patterns | Python's `glob` misreads a vault path containing `[`, `*` or `?` |
| In multi-vault mode the world is the folder under the vaults root | The original took the grandparent of the campaign folder, which finds nothing when `campaigns_subdir` has more than one component |
| `whisper_device` above 4294967295 is rejected; only ASCII digits are accepted | Python also accepts arbitrary-size and non-ASCII digits |
| Environment variables that are not valid UTF-8 are ignored | Paths are `&str` throughout; this is Linux and UTF-8 only |
| The TUI log is `ttrpg_tui.log` in the same directory, with UTC timestamps | Avoids interleaving with the GUI's log; no timezone database |
| An unconfigured TUI shows the full setup instructions in the details pane | The GUI showed a one-line hint |

### Milestone 2: the transcription run

| Difference | Why |
|---|---|
| A confirmation step names the folder, says the files are **moved**, and warns when a chosen file will replace one with the same name | The GUI moved files with no prompt and replaced same-name files silently |
| Start also works with nothing chosen when audio is already in the session folder | A retry needs no new files; the GUI had no explicit way to do it |
| The tool check is done in Rust (`ttrpg-tui check`, and at Start before anything is moved) | `transcribe.sh --check` creates folders and CSV files; the GUI also moved the audio before the script discovered a missing tool |
| One list of audio extensions (flac mp3 wav m4a ogg aac mp4 wma, any case) for the picker and the pipeline | The original had two lists that disagreed |
| The result is built from the state rows the run appended, plus the script's exit code and `PROGRESS:postdone` | The GUI printed "Complete!" whatever happened. Only rows appended during this run count, so a track is never credited with an earlier run's success |
| Cancel sends TERM, then KILL after 3 s, to the script's whole process group. The track in flight gets no state row, so the next run retries it | PORTING.md section 8 suggested recording "Error: cancelled"; the script owns that file, and a missing row already means "not done" |
| The session number is offered: the one after the latest session with a transcript, or the latest itself if it has none | The GUI left the field empty |
| Output from the script is stripped of control characters before it is shown | whisper-cli and ffmpeg write escape codes that would otherwise recolour or move the screen |
| `q` while a job runs warns first; a second `q` cancels the job and then quits | Quitting must not leave whisper-cli running |
| Tool locations are settings (`whisper_bin`, `whisper_model`, `legacy_dir`) | `transcribe.sh` took them as flags the GUI never passed |

### Milestone 3: unknown speakers

| Difference | Why |
|---|---|
| The key for resolving is `r` (PORTING.md's suggestion); rescanning moved to `R` and left the button bar | The button bar is full |
| New `discord_names` entries are laid out like the entries already in the file: the same indent, and spread over lines if the existing ones are. The original always wrote four spaces and one line | A hand-formatted file should not get one entry in a different style. Files laid out the original's way get identical bytes (tested) |
| The `discord_names` object is found by reading the JSON, so a nested object of the same name earlier in the file is not mistaken for it; a file with the key twice at the top level is refused | The original took the first `"key": {` anywhere in the text; its own check then aborted, so the result was a failed save |
| Windows line endings are kept, and new lines use them | The original read in text mode and rewrote the whole file with bare `\n` |
| The file is written to a temporary neighbour and renamed over | A crash cannot leave half a file |
| After splicing, every added entry is also checked to read back as given | One more check than the original's |
| A stem the file already lists, without a character or a GM role, is left alone and the status says so | The original reported an empty "Added to ..." and the track stayed unknown with no explanation. The script treats such a track as unknown, so the app does too |
| Esc with something typed asks first; saving an empty form says so instead of closing | The original dialog discarded silently |
| Resolving is refused while a job runs | The script reads the alias file for each track, so a change mid-run could label tracks differently |

### Milestone 4: Index to Memory and the alias report

| Difference | Why |
|---|---|
| The index job stops at the first step that fails and says which; the chunk file is kept when only the Chroma step failed | The GUI logged and carried on |
| An empty chunk file is a failure before anything is stored | PORTING.md 7.3 says to fail; the original checked nothing |
| "ChromaDB is not installed" gets the install command and the `python` setting, not a traceback | The usual first-run problem |
| The `python` setting (`GM_PYTHON`) picks the interpreter | The GUI used `sys.executable`, which has no analogue here |
| The campaign is reloaded from disk before indexing and before the report | The GM may have edited it since it was chosen (PORTING.md 7.3) |
| `i` and `a` are refused while any job runs | One job at a time |
| The alias report opens itself when it finishes | PORTING.md 7.3 |

### Milestone 5: the Rust pipeline

`transcribe.sh`, `transcript_to_chunks.py` and the alias report are now Rust. Each stage
was checked against the original on the same input, byte for byte: the per-track
filtering and collapsing, the merge, the consolidation, `transcript.md`, the chunk
JSONL, the alias report, and a whole run with a stub whisper. The files a run writes
(`transcription_log.csv`, `transcription_state.csv`, `archive/`, `temp/*.processed.tsv`
and the three transcripts) have the original's names and layout, so a session started by
one can be finished by the other. The differences:

| Difference | Why |
|---|---|
| The bar moves within a track, from whisper's own segment lines against the length of the converted audio | The original showed progress per track only. Whisper's lines are sparse in silence, so it can pause, but it never goes backwards |
| A file that replaces one of the same name is transcribed again even though the state file says the name succeeded | The original skipped it, so a corrected download kept the old track's transcript, silently |
| `F` at the confirmation (and `Options.force`) is `--force`; `p` is `--post-process-only` | They were flags the GUI never passed |
| `ignore_words` and `whisper_max_context` are settings | They were flags the GUI never passed |
| The per-speaker `temp/<speaker>_transcript.tsv` files and the `Speaker file:` log lines are gone | Nothing read them |
| A new `Whisper  :` line in the log banner says which build ran and on which device | One line to read when a run goes wrong |
| Double quotes in the file name, speaker and error text of a state row become single quotes | The original wrote them as they were, which breaks the CSV. A track named with a quote is therefore not recognised as done on the next run (it is transcribed again), as in the original |
| Merge ties are always broken in byte order (the C locale) | The original used whatever locale it ran in, so the order of simultaneous lines from different tracks depended on the machine |
| A speaker whose label is the word `speaker` is kept | The original's consolidation and Markdown loops skipped any row whose first field was `speaker`, to skip the header, and so dropped such a speaker's lines |
| The raw `<track>.json` left by an earlier run is removed before whisper runs | A run in which whisper exits 0 without writing could otherwise be taken for a success on the old file |
| Whisper output that is not valid JSON, or has no segments list, is an `Error` row ("Unreadable whisper output: ...") and a named failure | The original's `jq` failure ended the whole script under `set -e`, after the audio had been converted and nothing recorded |
| Failing to start `ffmpeg` or `whisper-cli` is logged with the system's reason | The original printed the shell's "No such file" into the CSV log only |
| The chunk file and the alias report are written by the program; their dates, ordering and rounding were checked against Python, including `difflib.SequenceMatcher` (reimplemented, since no crate matches its results) | Python is no longer needed for either |
| Tools needed to transcribe: `ffmpeg`, `whisper-cli` and the model. `bash`, `jq` and `python3` are not | Nothing runs a script any more. Index still needs the Python with ChromaDB |

Known limits of milestone 5:

- A real, short "Thank you." that directly follows a run of invented ones is collapsed
  into the run and dropped with it. That is the original's behaviour and the rule's
  documented limit (see the stock-phrase rule below).
- The progress inside a track needs whisper to print its segment lines to stdout, which
  `whisper-cli` does by default. A build that does not shows the per-track bar only.
- Reading the converted audio's length uses the WAV header; ffmpeg writing to a file
  always fills it in.

Known limits of milestone 4: ChromaDB itself is not tested here (no ChromaDB in the
test environment); the tests stand in for it and check what is passed to it and how
the app reads the answer. `query` and `info` against the store are not in the app.

Known limits of milestone 3: the form edits only `discord_names`. A character that is
not in `players` is accepted as typed (the original did the same), so a typo makes a
new character. The line-up of columns inside the file's entries is not copied: only
the indent and the line layout are.

Known limits of milestone 2:

- Progress was per track here; milestone 5 made it move within a track.
- Unknown speakers can be seen but not added from inside the app (milestone 3 does
  that). Such a track is labelled by its file name.
- `--force` and `--post-process-only` are `F` and `p` in milestone 5; `--cleanup` is not
  exposed (the temp folder is kept for a rebuild). `ignore_words` and `-mc` are
  settings; the two stock-phrase options are settings too, with `t` at the confirm step.
- A run still transcribes every audio file in the session folder, not only the ones
  chosen in this run, and the list in the app shows both.
- The picker lists only folders and audio files. A Craig `.zip` has to be unpacked first.

## The stock-phrase rule

Added in this repository, first to the pinned copy of `transcribe.sh` and now in Rust.
Whisper invents "Thank you." on a silent stretch of a track, one per 30 s window, and
the collapse step then joins the repeats into one long line. The default filter only
drops whole lines equal to `you silence um uh ah like right well`, so these stayed in.

In one real session (five tracks, 90 minutes) there were 136 such lines in the merged
file, 133 of them on the four player tracks and 3 on the GM track, which talks most.
In the final transcript they were 125 entries, 12% of all entries. Of the 136, 134 lasted 21 s or longer: 49 lasted 30 s, 24 lasted 60 s, 19 lasted
90 s, and the rest ran on in further multiples up to 540 s. The other two lasted 5 s
and 16 s. A person saying it takes a second or two. The rule therefore drops a line
that is exactly a stock phrase (same matching as the filler words: case ignored, one
trailing mark ignored) and lasts 20 s or more, measured after the collapse. On that
session it drops 134 of the 136 "Thank you." lines and 2 "We'll be right back." lines
and leaves every other line's start, end and text as they were.

Limits:

- It was derived from one session. Check a new session's count before trusting it.
- A real "Thank you." that follows a run of invented ones with nothing between is
  collapsed into the run and dropped with it.
- A real one that whisper stretched over a whole window is dropped.
- `stock_phrases` set to `off` turns the rule off; `stock_min_seconds` changes the
  length. Both are settings, and `t` at the confirmation turns it off for one run.

Tried and rejected: whisper.cpp `--vad`. It removed the invented lines and cut the run
time by 46%, but it stitches a speaker's bursts of speech together and stamps the
result from the first burst to the last. Segments of up to 925 s holding about 20
words appeared (the longest without VAD was 35 s). For the quietest player 33% of
words were stamped more than 30 s early; for the GM, 3.5%. The transcript stays valid
but quiet players' lines land in the wrong scenes.

Kept on purpose, because the original does it: the config file is read lazily and
only when a value needs it, so a broken `config.json` is not an error until then;
`whisper_device` reads the config even when the environment variable would win;
`vaults_root` wins over `vault`; empty environment variables count as unset; and
campaigns are ordered by full path string, so `main-old` lists before `main`.
