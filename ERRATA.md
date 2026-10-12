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

Known limits of milestone 2:

- Progress is per track. A single track shows a spinner and elapsed time. The real
  log shows a 73-minute track takes under 40 s, so this was left for later.
- Unknown speakers can be seen but not added from inside the app (milestone 3). The
  script labels such a track by its file name.
- `--force`, `--post-process-only`, `--cleanup`, `--ignore-words` and `-mc` are not
  exposed yet.
- `transcribe.sh` still transcribes every audio file in the session folder, not only
  the ones chosen in this run, and the list in the app shows both.
- The picker lists only folders and audio files. A Craig `.zip` has to be unpacked first.

Kept on purpose, because the original does it: the config file is read lazily and
only when a value needs it, so a broken `config.json` is not an error until then;
`whisper_device` reads the config even when the environment variable would win;
`vaults_root` wins over `vault`; empty environment variables count as unset; and
campaigns are ordered by full path string, so `main-old` lists before `main`.
