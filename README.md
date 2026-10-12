# ttrpg-tui

A terminal front end (ratatui, mouse enabled) for the TTRPG session transcriber.
It replaces the GTK GUI in `Kuntypilled/ttrpg-transcriber`, which stays untouched
until this one is verified. The specification is [PORTING.md](PORTING.md).
What was found to differ between that document and the code is in
[ERRATA.md](ERRATA.md).

## Status

Milestones 1 to 5 of 7 (PORTING.md section 11).

What you can do now:

- Pick a campaign. The session number is offered for you: the one after your last
  finished session, or the same one again if its run never produced a transcript.
- Choose the Craig audio files in a yazi-style picker (folders, multi-select).
- See who is speaking on every track, from the campaign's `discord_names`. Tracks
  with no entry are flagged, and `r` opens a form to say who they are: it adds them
  to the campaign's `canon_aliases.json` by splicing in the new lines, so the rest
  of the file is not touched.
- Press Start. After a confirmation that says plainly the files are **moved**, they
  go into the session's audio folder and the program converts each track with
  `ffmpeg`, runs `whisper-cli` on it, and builds the transcripts, behind a progress
  bar that moves through each track as whisper works. The output streams into a log
  (whisper's own lines dimmed). Cancel at any time. Tracks that already succeeded are
  skipped; `F` at the confirmation transcribes them again. A file that replaces one of
  the same name is always transcribed again.
- Whisper makes up "Thank you." on silent stretches of a track. The program drops
  those, and the confirmation step shows the rule and lets you keep them for one run
  (`t`). The rule is described in `ERRATA.md`.
- `p` (Rebuild) rewrites `merged_transcript.tsv`, `consolidated_transcript.tsv` and
  `transcript.md` from the whisper output saved in `archive/`. It needs no audio, no
  whisper and no `ffmpeg`, so a changed rule or a newly resolved speaker can be
  applied to a finished session. It rewrites `transcript.md`, and says so first.
- Read the result honestly ("4 of 5 transcribed, 1 failed", with the failing track
  named), mark each track done or failed in the list, and read the finished
  transcript in the app.
- Press `i` to index the session's transcript into the campaign's memory (the chunker,
  then ChromaDB, with a progress bar and the helpers' output in the log). A campaign
  with no `review_only` list is asked about first: run the alias report, index
  anyway, or cancel. `a` writes the alias report on its own and opens it; it never
  edits `canon_aliases.json`. A campaign without a `review_only` list carries a
  reminder in the session pane.
- Retry: with nothing chosen, the audio already in the session folder is the job,
  and tracks that already succeeded are skipped.
- The plain commands `doctor`, `list`, `paths`, `whisper-device` (same output as
  `campaign.py`) and `check` (are the tools installed? creates nothing).

Transcribing, the post-processing, the chunker and the alias report are all Rust now,
each checked byte for byte against the scripts they replaced. ChromaDB itself stays
Python for good: Index needs the Python that has ChromaDB installed (the `python`
setting). Not done yet: milestone 6 (cleanup: the Chroma helper made self-contained
and the `legacy/` folder removed) and milestone 7 (the cutover).

## Try it without whisper or real recordings

```sh
scripts/demo.sh
```

builds a throwaway world (an invented campaign, six short "Craig" tracks in a
Downloads folder, one from a speaker the campaign does not know, and a stand-in
`whisper-cli` that takes a few seconds per track) and opens the app on it. Nothing
outside the demo folder is touched (the vector store is pretended so ChromaDB is not
needed). Follow the hints it prints: `f`, `a`, `c`,
`s`, Enter; afterwards `r` resolves the unknown guest. Variations: `DEMO_FAIL=1` adds a track that fails, `DEMO_DELAY=0` makes
runs instant, `--keep` reuses last time's world so you can see skipped tracks.

## Install and run

You need Rust to build it (Arch: `sudo pacman -S rust`, or rustup), plus `whisper-cli`
and `ffmpeg` to transcribe, and a Python with ChromaDB to use Index (nothing else needs
Python, `bash` or `jq` any more). `ttrpg-tui check` says what is missing and what to
install.

```sh
cargo run --release -p ttrpg-tui              # the app
cargo run --release -p ttrpg-tui -- check     # tools
cargo run --release -p ttrpg-tui -- doctor    # where every folder resolved from
```

A terminal of at least 100 x 28 is comfortable. `cargo install --path crates/ttrpg-tui`
works too, as long as this folder stays where it is: Index finds `legacy/chroma_memory.py`
here, or wherever `legacy_dir` points.

### Configuration

The same as the original: environment variables, then
`$XDG_CONFIG_HOME/ttrpg-transcriber/config.json`, then built-in defaults. `doctor`
shows where each value came from; with no vault configured it prints the setup
instructions. These settings are new:

| Setting | Environment variable | Config key | Default |
|---|---|---|---|
| whisper command | `GM_WHISPER_BIN` | `whisper_bin` | `whisper-cli` on PATH |
| model file | `GM_WHISPER_MODEL` | `whisper_model` | `~/.local/share/whisper.cpp/models/ggml-large-v3-turbo.bin` |
| where the Chroma helper lives (for Index) | `GM_LEGACY_DIR` | `legacy_dir` | the `legacy/` folder next to the program, or in the checkout it was built from |
| Python that has ChromaDB (for Index) | `GM_PYTHON` | `python` | `python3` on PATH |
| phrases whisper makes up on silence | `GM_STOCK_PHRASES` (comma-separated, or `off`) | `stock_phrases` (a list, or `[]` for off) | `thank you`, `we'll be right back` |
| how long such a line must last to be dropped | `GM_STOCK_MIN_SECONDS` | `stock_min_seconds` | `20` |
| lines dropped as filler (the whole line, case ignored) | `GM_IGNORE_WORDS` (comma-separated, or `off`) | `ignore_words` (a list, or text) | `you`, `silence`, `um`, `uh`, `ah`, `like`, `right`, `well` |
| how much of the earlier text whisper sees (`-mc`; -1 is whisper's own default) | `GM_WHISPER_MC` | `whisper_max_context` | `0` |

`GM_WHISPER_DEVICE` / `whisper_device` (for machines with two GPUs) work as before.

## Keys

Everything has a key and a mouse equivalent: click a campaign, a tab, a button on
the bottom row, or the session number; the wheel moves or scrolls whatever is under
the pointer.

| Key | Does |
|---|---|
| `Tab`, `h`, `l` | move between the three panes |
| `j` `k`, arrows, `g` `G`, `PgUp` `PgDn` | move or scroll in the focused pane |
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

## Layout

| Path | What |
|---|---|
| `crates/ttrpg-core` | Library, no UI. Config, campaigns, paths, speaker resolution, the alias-file edit, the transcription run and its post-processing, the chunker, the alias report. |
| `crates/ttrpg-tui` | The `ttrpg-tui` program: commands, and the app as a library so tests can drive it. |
| `legacy/` | Copies of the original scripts, kept until milestone 6: they produced the golden test files, and `chroma_memory.py` still runs Index. See `legacy/README.md`. |
| `examples/` | Invented example config and alias file. |
| `scripts/demo.sh` | The demo above. |

## Test

```sh
cargo test --workspace
```

Four kinds of test:

- **Golden files from the originals**: the post-processing, the chunker, the alias
  report and a whole run (`ffmpeg`, a stub `whisper-cli`, five tone files) are
  compared byte for byte with what the original scripts wrote on the same input. The
  `generate.sh` next to each set of files says how it was made.
- **Parity**: the same commands through `legacy/campaign.py` and the Rust code must
  print the same thing, and the alias-file edit must produce the same bytes as the
  original's `append_object_entries`.
- **End to end**: the real Rust runner with real `ffmpeg` and a stub `whisper-cli`,
  driven through the job runner and through the whole app with keystrokes and mouse
  clicks, including cancel, retry, rebuild and every failure the run can report.
- **Screen**: each pane, tab and dialog is drawn into an in-memory terminal and read
  back, at many sizes.

The parity tests need `python3`; the end-to-end ones need `bash` (for the stub) and
`ffmpeg`. Without them the tests print `SKIPPED` and pass.

## Data

Fixtures and examples contain only invented names. Real campaign files,
transcripts, audio and logs do not belong in this repository.
