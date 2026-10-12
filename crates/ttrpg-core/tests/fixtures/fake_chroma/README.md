A stand-in for the `chromadb` package, just enough for `python/chroma_memory.py` to run
against it in tests. It keeps everything in `<store>/store.json` and ranks results by
shared words, so it says nothing about how real ChromaDB embeds or searches. What it does
let the tests check is the helper's own logic: the batches it upserts in, the filters it
builds, the collection settings it asks for and the text it prints.

Use it with `PYTHONPATH=<this folder>`.
