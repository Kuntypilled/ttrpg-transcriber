# legacy

Copies of files from `Kuntypilled/ttrpg-transcriber` at commit `98aa387`
(2026-10-11). All are byte-identical to that commit except `transcribe.sh`, which
carries the one local change listed below. Do not edit them otherwise.

| File | Why it is here |
|---|---|
| `campaign.py` | The parity oracle: the Rust port in `crates/ttrpg-core` is tested against it. Also imported by `chroma_memory.py`. |
| `transcribe.sh` | No longer run by the program (milestone 5). It made the golden files in `crates/ttrpg-core/tests/fixtures/{pipeline,run}/`. |
| `transcript_to_chunks.py` | Same, for `.../fixtures/chunker/` and `.../fixtures/report/`. |
| `chroma_memory.py` | Stays Python. Called as a subprocess by Index. |
| `requirements.txt` | Python dependencies of `chroma_memory.py`. |

## Local changes

`transcribe.sh`: a stock-phrase rule (`DEFAULT_STOCK_PHRASES`, `is_stock_segment`,
`--stock-phrases`, `--stock-min-seconds`). A line that is exactly "thank you" or
"we'll be right back" is dropped when it lasts 20 s or more, after duplicate lines
have been collapsed. Whisper invents these on silent stretches, one per 30 s window.
A run that drops nothing logs and outputs what the original did, plus one extra
`Stock    :` line in the banner that says which phrases are active. The
reasoning and the evidence are in `ERRATA.md`; the tests are
`crates/ttrpg-core/tests/stock_phrases.rs`. The original repo is unchanged.

`transcribe.sh` and `transcript_to_chunks.py` import or call `campaign.py` from
their own directory, so this folder must stay together. Delete it at milestone 6,
once `chroma_memory.py` has been made self-contained (PORTING.md section 6.1, option 2).
The scripts' behaviour is kept in the golden files, each with the `generate.sh` that
made it.
