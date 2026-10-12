# ttrpg-tui

A terminal front end (ratatui, mouse enabled) for the TTRPG session transcriber.
It replaces the GTK GUI in `Kuntypilled/ttrpg-transcriber`, which stays untouched
until this one is verified. The specification is [PORTING.md](PORTING.md).
What was found to differ between that document and the code is in
[ERRATA.md](ERRATA.md).

## Status

Milestone 1 of 7 (PORTING.md section 11): read-only skeleton.

- Config and root resolution, campaign discovery, the path rules and the campaign
  schema accessors, ported from `campaign.py` and tested against it.
- `doctor`, `list`, `paths` and `whisper-device` commands with the original's output.
- A TUI that lists campaigns and shows the path summary. Keys: `j`/`k`, `g`/`G`,
  `PgUp`/`PgDn`, `r` rescan, `?` help, `q`. Mouse: click a campaign, wheel to move
  or scroll.

Not started: the session field, file picker, transcription, speaker resolution,
indexing, and the Rust replacements of the shell and Python stages.

## Layout

| Path | What |
|---|---|
| `crates/ttrpg-core` | Library, no UI. Config, campaigns, paths, report text. |
| `crates/ttrpg-tui` | The `ttrpg-tui` binary: CLI commands and the ratatui app. |
| `legacy/` | Pinned copies of the original scripts. See `legacy/README.md`. |
| `examples/` | Invented example config and alias file. |

## Use

```sh
cargo run -p ttrpg-tui -- doctor          # what every root resolved to, and from where
cargo run -p ttrpg-tui -- list
cargo run -p ttrpg-tui -- paths --campaign <world>/<campaign> --session 13
cargo run -p ttrpg-tui                    # the TUI (needs a terminal)
```

Configuration is the same as the original: environment variables, then
`$XDG_CONFIG_HOME/ttrpg-transcriber/config.json`, then built-in defaults. Run
`doctor` first. With no vault configured it prints the setup instructions.

## Test

```sh
cargo test --workspace
```

`crates/ttrpg-core/tests/parity.rs` and `crates/ttrpg-tui/tests/cli.rs` run the same
commands through `legacy/campaign.py` and the Rust code and compare the output.
They need `python3`; without it they print `SKIPPED` and pass.

## Data

Fixtures and examples contain only invented names. Real campaign files,
transcripts, audio and logs do not belong in this repository.
