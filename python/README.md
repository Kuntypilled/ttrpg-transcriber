# python

`chroma_memory.py` is the one piece of Python left. ChromaDB and sentence-transformers
have no practical Rust equivalent that reads an existing store, so Index to Memory
hands the chunk file to this helper. The program carries a copy of the script inside
itself (it is built in), so nothing here has to be installed next to the binary; only
the two packages in `requirements.txt` have to be installed for the Python named by the
`python` setting.

It started as a copy of `chroma_memory.py` at `Kuntypilled/ttrpg-transcriber@98aa387` and
differs in one way: it no longer imports `campaign.py`. It knows nothing about
campaigns. The caller passes the database folder (`--db`) and, for `info`, the player
characters (`--pcs`). `ingest` takes the chunk file as an argument; `--campaign` and
`--session` are gone. `--db` still beats `$TTRPG_DB`, the collection, the embedding
model (`$TTRPG_EMBED`), the batch size and the output are unchanged.

By hand, `ttrpg-tui info CAMPAIGN` and `ttrpg-tui query CAMPAIGN "text"` work out the
paths and run it for you.

Tests: `crates/ttrpg-core/tests/chroma_helper.rs` runs the script against a stand-in
for `chromadb` (`tests/fixtures/fake_chroma`), so its own logic is checked without the
real package or the embedding model.
