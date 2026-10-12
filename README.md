# ttrpg-tui

A terminal front end (ratatui, mouse enabled) for the TTRPG session transcriber.
It replaces the GTK GUI in `Kuntypilled/ttrpg-transcriber`, which stays untouched
until this one is verified. The specification is [PORTING.md](PORTING.md).
What was found to differ between that document and the code is in
[ERRATA.md](ERRATA.md).

## Status

Milestones 1 to 4 of 7 (PORTING.md section 11).

What you can do now:

- Pick a campaign. The session number is offered for you: the one after your last
  finished session, or the same one again if its run never produced a transcript.
- Choose the Craig audio files in a yazi-style picker (folders, multi-select).
- See who is speaking on every track, from the campaign's `discord_names`. Tracks
  with no entry are flagged, and `r` opens a form to say who they are: it adds them
  to the campaign's `canon_aliases.json` by splicing in the new lines, so the rest
  of the file is not touched.
- Press Start. After a confirmation that says plainly the files are **moved**, they
  go into the session's audio folder and the original `transcribe.sh` runs behind a
  progress bar, with its output streaming into a log (whisper's own lines dimmed, the
  script's not). Cancel at any time.
- Whisper makes up "Thank you." on silent stretches of a track. The script drops
  those, and the confirmation step shows the rule and lets you keep them for one run
  (`t`). It is the stock-phrase rule in `legacy/README.md`.
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

Not done yet: replacing the shell and Python stages with Rust (milestone 5). Until
then the app runs the scripts in `legacy/`. ChromaDB itself stays Python for good:
it needs the Python that has ChromaDB installed (the `python` setting).

## Try it without whisper or real recordings

```sh
scripts/demo.sh
```

builds a throwaway world (an invented campaign, six tiny "Craig" tracks in a
Downloads folder, one from a speaker the campaign does not know, and a stand-in
`whisper-cli` that waits a few seconds per track) and opens the app on it. Nothing
outside the demo folder is touched (the vector store is pretended so ChromaDB is not
needed). Follow the hints it prints: `f`, `a`, `c`,
`s`, Enter; afterwards `r` resolves the unknown guest. Variations: `DEMO_FAIL=1` adds a track that fails, `DEMO_DELAY=0` makes
runs instant, `--keep` reuses last time's world so you can see skipped tracks.

## Install and run

You need Rust to build it (Arch: `sudo pacman -S rust`, or rustup) and the same
tools the original pipeline uses: `whisper-cli`, `ffmpeg`, `jq`, `python3`, `bash`.
`ttrpg-tui check` says what is missing and what to install.

```sh
cargo run --release -p ttrpg-tui              # the app
cargo run --release -p ttrpg-tui -- check     # tools
cargo run --release -p ttrpg-tui -- doctor    # where every folder resolved from
```

A terminal of at least 100 x 28 is comfortable. `cargo install --path crates/ttrpg-tui`
works too, as long as this folder stays where it is: the program finds `legacy/`
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
| where `transcribe.sh` lives | `GM_LEGACY_DIR` | `legacy_dir` | the `legacy/` folder next to the program, or in the checkout it was built from |
| Python that has ChromaDB (for Index) | `GM_PYTHON` | `python` | `python3` on PATH |
| phrases whisper makes up on silence | `GM_STOCK_PHRASES` (comma-separated, or `off`) | `stock_phrases` (a list, or `[]` for off) | `thank you`, `we'll be right back` |
| how long such a line must last to be dropped | `GM_STOCK_MIN_SECONDS` | `stock_min_seconds` | `20` |

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
| `s` | start (asks first; at the question, `t` keeps or drops whisper's made-up lines for this run) |
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
| `crates/ttrpg-core` | Library, no UI. Config, campaigns, paths, speaker resolution, the alias-file edit, the job runner. |
| `crates/ttrpg-tui` | The `ttrpg-tui` program: commands, and the app as a library so tests can drive it. |
| `legacy/` | Copies of the original scripts, with one documented change. See `legacy/README.md`. |
| `examples/` | Invented example config and alias file. |
| `scripts/demo.sh` | The demo above. |

## Test

```sh
cargo test --workspace
```

Three kinds of test:

- **Parity**: the same commands through `legacy/campaign.py` and the Rust code must
  print the same thing, and the alias-file edit must produce the same bytes as the
  original's `append_object_entries`.
- **End to end**: the real `legacy/transcribe.sh`, real `ffmpeg` and `jq`, and a stub
  `whisper-cli`, driven through the job runner and through the whole app with
  keystrokes and mouse clicks.
- **Screen**: each pane, tab and dialog is drawn into an in-memory terminal and read
  back, at many sizes.

The first two need `python3` (and `bash`, `ffmpeg`, `jq`); without them the tests
print `SKIPPED` and pass.

## Data

Fixtures and examples contain only invented names. Real campaign files,
transcripts, audio and logs do not belong in this repository.
