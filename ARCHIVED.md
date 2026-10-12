# The GTK GUI is archived

The GTK3 front end in this repository (`transcribe_gui.py`, started by
`launch_transcriber.sh`) is replaced by a terminal program written in Rust:

**https://github.com/Kuntypilled/ttrpg_transcriber_tui**

## What this branch changes

Only this file and a notice at the top of `README.md`. Nothing is moved, renamed or
deleted, so the GUI and every script here still run exactly as they did at commit
`98aa387`. `main` is untouched. The branch is there to merge on the day the terminal
program has earned the place, and not before.

## Where each file went

| Here | In the new repository |
|---|---|
| `transcribe_gui.py` | the app, `crates/ttrpg-tui` |
| `launch_transcriber.sh`, `ttrpg-transcriber.desktop.example` | run `ttrpg-tui` in a terminal (`cargo run --release -p ttrpg-tui`, or `cargo install --path crates/ttrpg-tui`) |
| `transcribe.sh` | `crates/ttrpg-core/src/transcribe.rs` and `pipeline.rs`; it runs `ffmpeg` and `whisper-cli` itself, so `bash` and `jq` are no longer needed |
| `transcript_to_chunks.py` | `crates/ttrpg-core/src/chunker.rs` and `alias_report.rs` |
| `campaign.py` | `campaign.rs`, `settings.rs`, `aliases.rs`, `report.rs` |
| `chroma_memory.py` | `python/chroma_memory.py`: still Python, but it no longer imports `campaign.py`; it takes `--db` and `--pcs`, and `ttrpg-tui info` / `ttrpg-tui query` fill those in |
| `requirements.txt` | `python/requirements.txt` (`chromadb`, `sentence-transformers` only) |
| `layout-master.sh`, `layout-3x2-horizontal.sh` | unrelated to the pipeline; they stay here |

## What carries over unchanged

The configuration: the same environment variables, the same
`~/.config/ttrpg-transcriber/config.json`, the same `canon_aliases.json` in each
campaign. The same folders for audio, transcripts, chunks, working files and the vector
store, so the existing Chroma stores are used as they are (same collection, same
embedding model, nothing is re-embedded). The post-processing, the chunk JSONL and the
alias report were checked against these scripts byte for byte on invented data.

## What is different

The new repository's `ERRATA.md` lists every deliberate difference. The ones you will
notice: files are moved only after a confirmation that says so; a run reports "4 of 5
transcribed, 1 failed" instead of "Complete!"; whisper's invented "Thank you." lines are
dropped by a stated rule; a finished session's transcript can be rebuilt from the saved
whisper output (`p`) without audio.

## Before merging this branch

The Rust pipeline has been run against stand-ins for whisper and ChromaDB and against
these scripts' own output on invented data. It has not yet been run on a real session
with real whisper and real ChromaDB. Run one session through the terminal program,
compare its `transcript.md` and chunk file with what the GUI made from the same audio,
and run `ttrpg-tui info CAMPAIGN` once against a real store.

## Finishing the archive (your call, none of it is done here)

1. Merge this branch.
2. Optionally move `transcribe_gui.py`, `launch_transcriber.sh` and the `.desktop` example
   into an `archive/` folder. Do that only after your launcher and any desktop entry no
   longer point at them.
3. When you no longer need the old scripts, use GitHub's *Settings > Archive this
   repository*, which makes it read-only.
