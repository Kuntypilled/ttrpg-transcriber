# Differences from the earlier version

This program replaces a GTK3 front end driven by a bash script and three Python scripts
(the earlier version; its files are in `archive/`). This page records where the
behaviour differs on purpose, the bugs the earlier version had, and the limits that
remain. Statements about the earlier version were established by running it on invented
data with a stand-in `whisper-cli` that writes canned JSON, by reading its code, or, where
marked, against a real session's log and alias file (neither is stored here).

## Bugs in the earlier version

1. **`--post-process-only` replay re-strips the extension.** The speaker lookup was given a
   name that had already lost its extension and stripped from the last dot again. A username
   containing a dot was labelled correctly on a fresh run and wrongly on a replay
   (`2-some.user_0.flac`: fresh `Character_Name`, replay `2-some`).
2. **A skipped file was decided by a substring.** The state row was split on commas and the
   rest of the row had to contain `"Success"`. State rows are only appended, so one old
   Success row won forever: after a failed `--force` rerun the file held Success and Error
   rows for the same tracks, and the next normal run skipped them all.
3. **`--check` created files.** It ran after the folder and CSV creation, so checking an empty
   directory created `transcriptions/archive`, `transcriptions/temp` and both CSVs, and with a
   campaign and session it would create the session folder in the vault. `ttrpg-tui check`
   creates nothing.
4. **Empty-text rows broke the chunker.** When an empty-text row was its own entry in
   `transcript.md`, the chunker's regex took the next speaker's header and text as its body.
   Whether real whisper.cpp output contains whitespace-only segments is not known.
5. **Unmatched speakers were labelled `GM:` in chunk text** while their role stayed Player.
6. **The GUI and the script resolved speakers differently.** An entry with `role: "Player"`
   and a null `character` showed as "Player (name)" in the GUI and fell back to the file name
   in the script. There is one function now.
7. **`shutil.move` overwrote silently** when a same-name file was already in the session audio
   folder. After a run the GUI kept the old source paths selected, so pressing Start again
   failed at the move until the files were chosen again from the audio folder.
8. **The chunk `date` was the day Index was clicked**, not the session date. This program
   keeps that behaviour, because the dates in existing chunk files depend on it.
9. **The state CSV was quoted without escaping.** The log was not quoted at all, and
   whisper-cli's stderr was redirected into the same file: in a real log, 265 of 303 non-blank
   lines were whisper output, not log rows.
10. **`discord_names` key rule was documented wrongly.** The code was right: the key is
    everything after the first `-`, minus the extension, matched exactly. Craig does not add a
    trailing `_M`, and a real alias file uses bare usernames. The comment in
    `canon_aliases.example.json` was wrong; `examples/canon_aliases.example.json` has it
    corrected.
11. **Index was not re-enabled after a successful index**, so it stayed disabled until the
    session or campaign changed.
12. **Settings that existed only as command-line flags**: the model path, the whisper binary,
    the filler words and `-mc`. They are settings now.

## Configuration and paths

| Difference | Why |
|---|---|
| Messages say `ttrpg-tui ...` instead of `python3 campaign.py ...`, and the example campaign id is `emberfall/main` | This is a different program |
| The text of a JSON syntax error is `serde_json`'s | Python's wording cannot be reproduced; the file is still named |
| A config or alias value of the wrong JSON type where a path or string is expected (array, object), or an alias file that is not an object, is an error naming the file and key | The earlier version crashed or built a nonsense path |
| A config file that exists but cannot be read is an error naming it | The earlier version crashed |
| Root paths are used literally when scanning for campaigns, not as glob patterns | Python's `glob` misreads a vault path containing `[`, `*` or `?` |
| In multi-vault mode the world is the folder under the vaults root | The earlier version took the grandparent of the campaign folder, which finds nothing when the campaigns subfolder has more than one component |
| `whisper_device` above 4294967295 is rejected; only ASCII digits are accepted | Python also accepts arbitrary-size and non-ASCII digits |
| Environment variables that are not valid UTF-8 are ignored | Paths are `&str` throughout; this is Linux and UTF-8 only |
| The program's log is `ttrpg_tui.log` in the same directory the GUI's log used, with UTC timestamps | The two never interleave; no timezone database is needed |
| An unconfigured program shows the full setup instructions in the details pane | The GUI showed a one-line hint |
| `menu_label` is not ported; the UI shows the label and the system separately | The earlier separator was an em dash |

Kept on purpose, because the earlier version does it: the config file is read lazily and only
when a value needs it, so a broken `config.json` is not an error until then; `whisper_device`
reads the config even when the environment variable would win; `vaults_root` wins over
`vault`; empty environment variables count as unset; and campaigns are ordered by full path
string, so `main-old` lists before `main`.

## The transcription run

`transcribe.sh` is Rust now. Each stage was checked against the script on the same input,
byte for byte: the per-track filtering and collapsing, the merge, the consolidation,
`transcript.md`, and a whole run with a stand-in whisper. The files a run writes
(`transcription_log.csv`, `transcription_state.csv`, `archive/`, `temp/*.processed.tsv` and
the three transcripts) keep the script's names and layout. A real session transcribed by this
program matched the stock-rule version of the script byte for byte.

| Difference | Why |
|---|---|
| A confirmation step names the folder, says the files are **moved**, and warns when a chosen file will replace one with the same name | The GUI moved files with no prompt and replaced same-name files silently |
| Start also works with nothing chosen when audio is already in the session folder | A retry needs no new files; the GUI had no explicit way to do it |
| The tool check is done in Rust (`ttrpg-tui check`, and at Start before anything is moved). Transcribing needs `ffmpeg`, `whisper-cli` and the model; `bash`, `jq` and `python3` are not needed | The GUI moved the audio before the script discovered a missing tool |
| One list of audio extensions (flac mp3 wav m4a ogg aac mp4 wma, any case) for the picker and the pipeline | The earlier version had two lists that disagreed |
| The result is built from what the run did, and says so ("4 of 5 transcribed, 1 failed", with the failing track named) | The GUI printed "Complete!" whatever happened |
| Cancel sends TERM, then KILL after 3 s, to the whole process group of the child. The track in flight gets no state row, so the next run retries it | A missing row already means "not done" |
| The session number is offered: the one after the latest session with a transcript, or the latest itself if it has none | The GUI left the field empty |
| Output from the children is stripped of control characters before it is shown | whisper-cli and ffmpeg write escape codes that would otherwise recolour or move the screen |
| `q` while a job runs warns first; a second `q` cancels the job and then quits | Quitting must not leave whisper-cli running |
| The bar moves within a track, from whisper's own segment lines against the length of the converted audio | The earlier version showed progress per track only. Whisper's lines are sparse in silence, so the bar can pause, but it never goes backwards |
| A file that replaces one of the same name is transcribed again even though the state file says the name succeeded | The earlier version skipped it, so a corrected download kept the old track's transcript, silently |
| `F` at the confirmation is `--force`; `p` (Rebuild) is `--post-process-only` | They were flags the GUI never passed |
| `whisper_bin`, `whisper_model`, `ignore_words` and `whisper_max_context` are settings | They were flags the GUI never passed |
| The per-speaker `temp/<speaker>_transcript.tsv` files and the `Speaker file:` log lines are gone | Nothing read them |
| A `Whisper  :` line in the log banner says which build ran and on which device | One line to read when a run goes wrong |
| Double quotes in the file name, speaker and error text of a state row become single quotes | The earlier version wrote them as they were, which breaks the CSV. A track named with a quote is therefore not recognised as done on the next run (it is transcribed again), as before |
| Merge ties are always broken in byte order (the C locale) | The script used whatever locale it ran in, so the order of simultaneous lines from different tracks depended on the machine |
| A speaker whose label is the word `speaker` is kept | The script's consolidation and Markdown loops skipped any row whose first field was `speaker`, to skip the header, and so dropped such a speaker's lines |
| The raw `<track>.json` left by an earlier run is removed before whisper runs | A run in which whisper exits 0 without writing could otherwise be taken for a success on the old file |
| Whisper output that is not valid JSON, or has no segments list, is an `Error` row ("Unreadable whisper output: ...") and a named failure | The script's `jq` failure ended the whole run under `set -e`, after the audio had been converted and nothing recorded |
| Failing to start `ffmpeg` or `whisper-cli` is logged with the system's reason | The script printed the shell's "No such file" into the CSV log only |

## Unknown speakers

| Difference | Why |
|---|---|
| `r` resolves unknown speakers; rescanning is `R` and has no button | The button bar is full |
| New `discord_names` entries are laid out like the entries already in the file: the same indent, and spread over lines if the existing ones are. The earlier version always wrote four spaces and one line | A hand-formatted file should not get one entry in a different style. Files laid out the earlier way get identical bytes (tested) |
| The `discord_names` object is found by reading the JSON, so a nested object of the same name earlier in the file is not mistaken for it; a file with the key twice at the top level is refused | The earlier version took the first `"key": {` anywhere in the text; its own check then aborted, so the result was a failed save |
| Windows line endings are kept, and new lines use them | The earlier version read in text mode and rewrote the whole file with bare `\n` |
| The file is written to a temporary neighbour and renamed over | A crash cannot leave half a file |
| After splicing, every added entry is also checked to read back as given | One more check than before |
| A stem the file already lists, without a character or a GM role, is left alone and the status says so | The earlier version reported an empty "Added to ..." and the track stayed unknown with no explanation. The pipeline treats such a track as unknown, so the app does too |
| Esc with something typed asks first; saving an empty form says so instead of closing | The earlier dialog discarded silently |
| Resolving is refused while a job runs | The run reads the alias file for each track, so a change mid-run could label tracks differently |

## Index to Memory, the alias report and the memory store

The chunker and the alias report are Rust. The chunk JSONL and the report text were checked
against the Python originals byte for byte, including a port of `difflib.SequenceMatcher`
(no crate gives the same results). Python's `json.dumps` writes `", "` and `": "`
separators and escapes all non-ASCII, and `serde_json` does neither by default, so a
formatter that copies Python's output is used.

| Difference | Why |
|---|---|
| The index job stops at the first step that fails and says which; the chunk file is kept when only the Chroma step failed | The GUI logged and carried on |
| An empty chunk file is a failure before anything is stored | The earlier version checked nothing |
| "ChromaDB is not installed" gets the install command and the `python` setting, not a traceback | The usual first-run problem |
| The `python` setting (`GM_PYTHON`) picks the interpreter | The GUI used `sys.executable`, which has no analogue here |
| The campaign is reloaded from disk before indexing and before the report | The GM may have edited it since it was chosen |
| `i` and `a` are refused while any job runs | One job at a time |
| The alias report opens itself when it finishes | A person wants to read it |
| `chroma_memory.py` no longer imports `campaign.py`. It takes `--db` and, for `info`, `--pcs`; `ingest` takes the chunk file as an argument. `--campaign` and `--session` are gone from it | The program already knows every path. One source of truth for them |
| The helper is built into the program and run with `python -c`; there is no file to find | `cargo install` works anywhere |
| `$TTRPG_DB` still moves the store, but the program reads it and passes `--db` | The earlier GUI passed no `--db`, so the helper read the variable. Same behaviour, now in one place |
| `ttrpg-tui info CAMPAIGN` and `ttrpg-tui query CAMPAIGN "text"` | The GUI never exposed them, and with the campaign import gone they would otherwise need the paths worked out by hand |
| `check` and the Tools dialog probe the `python` setting for `chromadb` and `sentence-transformers` (optional item "ChromaDB for Index") | A missing package is the usual first-run failure; it can now be seen before pressing `i` |
| Index logs a warning with a count when lines of the transcript belong to no speaker entry (notes above the first speaker line, a header with a typo) | The chunker dropped them without a word. The count does not change what is indexed |

## Limits

- Real ChromaDB and the embedding model are not exercised by any test (neither is installed
  in the test environment). The helper's own logic is, against a stand-in that records what
  it was asked to do; whether a real store accepts those calls depends on the installed
  `chromadb`, and the `configuration={"hnsw": ...}` keyword needs a recent release (see
  `python/requirements.txt`). Run `ttrpg-tui info` on a real campaign once to confirm.
- A real, short "Thank you." that directly follows a run of invented ones is collapsed into
  the run and dropped with it (see the stock-phrase rule below).
- The progress inside a track needs whisper to print its segment lines to stdout, which
  `whisper-cli` does by default. A build that does not shows the per-track bar only.
- The resolve form edits only `discord_names`. A character that is not in `players` is
  accepted as typed, so a typo makes a new character. The line-up of columns inside the
  file's entries is not copied: only the indent and the line layout are.
- A run transcribes every audio file in the session folder, not only the ones chosen in this
  run, and the list in the app shows both.
- The picker lists only folders and audio files. A Craig `.zip` has to be unpacked first.
- `--cleanup` is not exposed (the temp folder is kept so a rebuild can use it).

Observed on one real session: five tracks of about 73 minutes transcribed in 19 to 38 s each
on a GPU, and the whole session took about two minutes. A long track looks frozen for under
40 s.

## The stock-phrase rule

Whisper invents "Thank you." on a silent stretch of a track, one per 30 s window, and the
collapse step then joins the repeats into one long line. The default filter only drops whole
lines equal to `you silence um uh ah like right well`, so these stayed in.

In one real session (five tracks, 90 minutes) there were 136 "Thank you." lines in the merged file,
133 of them on the four player tracks and 3 on the GM track, which talks most. In the final
transcript they were 125 entries, 12% of all entries. Of the 136, 134 lasted 21 s or longer:
49 lasted 30 s, 24 lasted 60 s, 19 lasted 90 s, and the rest ran on in further multiples up
to 540 s. The other two lasted 5 s and 16 s. A person saying it takes a second or two. The
rule therefore drops a line that is exactly a stock phrase (same matching as the filler
words: case ignored, one trailing mark ignored) and lasts 20 s or more, measured after the
collapse. On that session it drops 134 of the 136 "Thank you." lines and 2 "We'll be right
back." lines (136 in all) and leaves every other line's start, end and text as they were. A
second run of the same audio through the Rust pipeline made the same
merged transcript and `transcript.md` as the script with the rule, and differed from a run
without the rule only by those 136 dropped lines.

Limits:

- It was derived from one session. Check a new session's count before trusting it.
- A real "Thank you." that follows a run of invented ones with nothing between is collapsed
  into the run and dropped with it.
- A real one that whisper stretched over a whole window is dropped.
- `stock_phrases` set to `off` turns the rule off; `stock_min_seconds` changes the length.
  Both are settings, and `t` at the confirmation turns it off for one run.

Tried and rejected: whisper.cpp `--vad`. It removed the invented lines and cut the run time
by 46%, but it stitches a speaker's bursts of speech together and stamps the result from the
first burst to the last. Segments of up to 925 s holding about 20 words appeared (the
longest without VAD was 35 s). For the quietest player 33% of words were stamped more than
30 s early; for the GM, 3.5%. The transcript stays valid but quiet players' lines land in the
wrong scenes.
