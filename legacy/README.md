# legacy

Verbatim copies of files from `Kuntypilled/ttrpg-transcriber` at commit
`98aa387` (2026-10-11), byte-identical to that commit. Do not edit them.

| File | Why it is here |
|---|---|
| `campaign.py` | The parity oracle: the Rust port in `crates/ttrpg-core` is tested against it. |
| `transcribe.sh` | Called by the TUI from milestone 2 on, until the Rust pipeline replaces it. |
| `transcript_to_chunks.py` | Same, until the Rust chunker replaces it. |
| `chroma_memory.py` | Stays Python. Called as a subprocess. |
| `requirements.txt` | Python dependencies of `chroma_memory.py`. |

`transcribe.sh` and `transcript_to_chunks.py` import or call `campaign.py` from
their own directory, so this folder must stay together. Delete it at milestone 6,
once the shell scripts are retired and `chroma_memory.py` has been made
self-contained (PORTING.md section 6.1, option 2).
