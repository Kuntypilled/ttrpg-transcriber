#!/usr/bin/env python3
"""
Stage 3 of the evidence layer: load chunk JSONL into a persistent Chroma
collection with LOCAL embeddings, and query it. One database per campaign.

This is the layer the MCP server (and you, by hand) will call. It holds RAW
TRANSCRIPTS ONLY. The bible and the per-session recaps stay OUT of here and
remain the canon layer that always wins on conflict.

ONE DATABASE PER CAMPAIGN, not one database with a campaign metadata field.
Two campaigns can share a world and still not share knowledge: a query run for
one table must not surface the other table's transcript. A
metadata filter is a thing you can forget to apply. A separate database is not.
It also means a corrupt index costs one campaign instead of all of them.

The store lives OUTSIDE the vault, under $GM_CHROMA_ROOT/<campaign>/, which
defaults to $XDG_DATA_HOME/ttrpg_memory/<campaign>/,
because it is a binary SQLite file plus an HNSW index: Obsidian Sync would push
megabytes on every ingest, two devices writing it has no locking and can corrupt
it, and it is fully regenerable from the JSONL, which is text and does live in
the vault.

This helper knows nothing about campaigns. The caller (ttrpg-tui) works out the
database path and the player characters and passes them in. By hand, the easy way
is the program's own commands, which fill those in for you:
  ttrpg-tui query my_campaign "the reveal at the docks" --from 13 --char Someone
  ttrpg-tui info my_campaign

Setup (on a machine where model weights can download):
  pip install -r requirements.txt
  # or, for a system Python that refuses non-venv installs:
  pip install --user --break-system-packages -r requirements.txt

Ingest:
  python3 chroma_memory.py ingest --db path/to/store path/to/chunks.jsonl
Query:
  python3 chroma_memory.py query --db path/to/store "the reveal at the docks" --from 13 --char Someone
  python3 chroma_memory.py query --db path/to/store "what the villain said" --contains Villain
Where does it live / what is in it:
  python3 chroma_memory.py info --db path/to/store --pcs "Someone,Someone Else"

--db is required unless $TTRPG_DB is set; --db wins when both are given.
"""
import argparse
import json
import os
import re
import sys

COLLECTION = "transcripts"
# bge-base is a solid retrieval default. Swap to bge-large-en-v1.5 for quality,
# or all-MiniLM-L6-v2 for speed. Do NOT mix models in one collection: the vector
# space changes and old vectors stop matching new queries. If the model changes,
# the database is invalid and has to be rebuilt. The JSONL is not, which is why
# the JSONL is the thing that lives in the vault.
EMBED_MODEL = os.environ.get("TTRPG_EMBED", "BAAI/bge-base-en-v1.5")


def normalize_character(name):
    """The identity used in metadata keys: spaces become underscores, so has_<PC> stays one token."""
    if name is None:
        return None
    return re.sub(r"\s+", "_", str(name).strip())


def resolve_db(override=None):
    """Explicit --db beats $TTRPG_DB. None when neither is given."""
    chosen = override or os.environ.get("TTRPG_DB")
    if not chosen:
        return None
    return os.path.abspath(os.path.expanduser(chosen))


def get_collection(db_path):
    import chromadb
    from chromadb.utils import embedding_functions
    os.makedirs(db_path, exist_ok=True)
    client = chromadb.PersistentClient(path=db_path)
    ef = embedding_functions.SentenceTransformerEmbeddingFunction(model_name=EMBED_MODEL)
    return client.get_or_create_collection(
        name=COLLECTION, embedding_function=ef,
        configuration={"hnsw": {"space": "cosine"}})


def ingest(db_path, jsonl_path):
    col = get_collection(db_path)
    ids, docs, metas = [], [], []
    for line in open(jsonl_path, encoding="utf-8"):
        line = line.strip()
        if not line:
            continue
        r = json.loads(line)
        ids.append(r["id"]); docs.append(r["document"]); metas.append(r["metadata"])
    # upsert so re-running a corrected transcript overwrites instead of duplicating
    B = 100
    for i in range(0, len(ids), B):
        col.upsert(ids=ids[i:i+B], documents=docs[i:i+B], metadatas=metas[i:i+B])
    print(f"Upserted {len(ids)} chunks into {db_path}. Collection now holds {col.count()}.")


def query(db_path, text, session_from=None, char=None, contains=None, k=6):
    col = get_collection(db_path)
    conds = []
    if session_from is not None:
        conds.append({"session": {"$gte": session_from}})
    if char:
        conds.append({f"has_{normalize_character(char)}": True})
    where = None if not conds else (conds[0] if len(conds) == 1 else {"$and": conds})
    res = col.query(query_texts=[text], n_results=k, where=where,
                    where_document=({"$contains": contains} if contains else None))
    if not res["documents"] or not res["documents"][0]:
        print(f"No results in {db_path}.")
        return
    for doc, meta, dist in zip(res["documents"][0], res["metadatas"][0], res["distances"][0]):
        print(f"\n=== s{meta['session']} {meta['start']}-{meta['end']} "
              f"[{meta['characters']}] cos_dist={dist:.3f} ===")
        print(doc[:600])


def info(db_path, pcs):
    print(f"db path    : {db_path}")
    print(f"exists     : {os.path.isdir(db_path)}")
    print(f"embeddings : {EMBED_MODEL}")
    print(f"pcs        : {', '.join(pcs) or '(none)'}")
    if not os.path.isdir(db_path):
        print("chunks     : 0 (no store yet)")
        return
    col = get_collection(db_path)
    print(f"chunks     : {col.count()}")
    got = col.get(include=["metadatas"])
    sessions = sorted({m.get("session") for m in got["metadatas"] if m.get("session") is not None})
    print(f"sessions   : {', '.join(str(s) for s in sessions) or '(none)'}")


def main(argv=None):
    ap = argparse.ArgumentParser(prog="chroma_memory", description="Per-campaign transcript vector store.")
    sub = ap.add_subparsers(dest="cmd", required=True)

    def common(p):
        p.add_argument("--db", help="the database folder (default: $TTRPG_DB)")

    pi = sub.add_parser("ingest")
    common(pi)
    pi.add_argument("jsonl", help="chunk JSONL to load")

    pq = sub.add_parser("query")
    common(pq)
    pq.add_argument("text")
    pq.add_argument("--from", dest="session_from", type=int)
    pq.add_argument("--char"); pq.add_argument("--contains")
    pq.add_argument("-k", type=int, default=6)

    pn = sub.add_parser("info")
    common(pn)
    pn.add_argument("--pcs", default="", help="player characters, comma-separated (shown only)")

    a = ap.parse_args(argv)

    db_path = resolve_db(a.db)
    if not db_path:
        sys.stderr.write("ERROR: give --db, or set $TTRPG_DB.\n")
        return 2

    if a.cmd == "ingest":
        if not os.path.isfile(a.jsonl):
            sys.stderr.write(f"ERROR: chunk file not found: {a.jsonl}\n")
            return 2
        ingest(db_path, a.jsonl)
    elif a.cmd == "query":
        query(db_path, a.text, a.session_from, a.char, a.contains, a.k)
    else:
        info(db_path, [p.strip() for p in a.pcs.split(",") if p.strip()])
    return 0


if __name__ == "__main__":
    sys.exit(main())
