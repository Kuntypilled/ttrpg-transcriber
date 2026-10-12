# oracle

The original program's scripts, kept for the tests and for nothing else. The program
does not run, read or need any of them.

They are copies of files from `Kuntypilled/ttrpg-transcriber` at commit `98aa387`
(2026-10-11). `campaign.py` and `transcript_to_chunks.py` are byte-identical to that
commit; `transcribe.sh` carries the one local change listed below. Do not edit them
otherwise.

| File | What it is for |
|---|---|
| `campaign.py` | The live oracle for `parity.rs`, `aliases_parity.rs` and the CLI test: the Rust port of the config, path and alias-file code is run against it on the same input. |
| `transcribe.sh` | Made the golden files in `../fixtures/pipeline/` and `../fixtures/run/`. |
| `transcript_to_chunks.py` | Made the golden files in `../fixtures/chunker/` and `../fixtures/report/`. |

The golden files are checked in, so the tests that use them need none of this. The
`generate.sh` next to each set says how it was made and can remake it (it needs bash,
jq, ffmpeg and python3).

## Local change

`transcribe.sh`: a stock-phrase rule (`DEFAULT_STOCK_PHRASES`, `is_stock_segment`,
`--stock-phrases`, `--stock-min-seconds`). A line that is exactly "thank you" or
"we'll be right back" is dropped when it lasts 20 s or more, after duplicate lines
have been collapsed. Whisper invents these on silent stretches, one per 30 s window.
A run that drops nothing logs and outputs what the original did, plus one extra
`Stock    :` line in the banner that says which phrases are active. The reasoning and
the evidence are in `docs/differences.md`; the Rust rule is tested in
`crates/ttrpg-core/tests/stock_phrases.rs`. The original repository is unchanged.

`transcribe.sh` and `transcript_to_chunks.py` import or call `campaign.py` from their
own directory, so this folder must stay together.
