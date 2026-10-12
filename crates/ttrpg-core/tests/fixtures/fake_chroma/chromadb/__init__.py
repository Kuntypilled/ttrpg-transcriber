"""Test double for chromadb (see ../README.md)."""
import json
import os
import re
import time


def _matches(where, meta):
    if where is None:
        return True
    if "$and" in where:
        return all(_matches(w, meta) for w in where["$and"])
    ((key, cond),) = where.items()
    value = meta.get(key)
    if isinstance(cond, dict):
        ((op, arg),) = cond.items()
        if op == "$gte":
            return value is not None and value >= arg
        raise NotImplementedError(op)
    return value == cond


def _words(text):
    return set(re.findall(r"\w+", text.lower()))


class _Collection:
    def __init__(self, path, name, embedding_function, configuration):
        self.file = os.path.join(path, "store.json")
        if os.path.exists(self.file):
            self.data = json.load(open(self.file, encoding="utf-8"))
        else:
            self.data = {"docs": {}, "batches": []}
        self.data["name"] = name
        self.data["model"] = embedding_function.model_name
        self.data["configuration"] = configuration

    def _save(self):
        json.dump(self.data, open(self.file, "w", encoding="utf-8"))

    def upsert(self, ids, documents, metadatas):
        time.sleep(float(os.environ.get("FAKE_CHROMA_SLEEP", "0")))
        self.data["batches"].append(len(ids))
        for i, d, m in zip(ids, documents, metadatas):
            self.data["docs"][i] = {"document": d, "metadata": m}
        self._save()

    def count(self):
        return len(self.data["docs"])

    def get(self, include=None):
        return {"metadatas": [v["metadata"] for v in self.data["docs"].values()]}

    def query(self, query_texts, n_results, where=None, where_document=None):
        self.data["last_query"] = {"n": n_results, "where": where, "where_document": where_document}
        self._save()
        wanted = _words(query_texts[0])
        scored = []
        for v in self.data["docs"].values():
            if not _matches(where, v["metadata"]):
                continue
            if where_document and where_document["$contains"] not in v["document"]:
                continue
            have = _words(v["document"])
            union = wanted | have
            dist = 1 - (len(wanted & have) / len(union) if union else 0)
            scored.append((dist, v))
        scored.sort(key=lambda t: t[0])
        scored = scored[:n_results]
        return {
            "documents": [[v["document"] for _, v in scored]],
            "metadatas": [[v["metadata"] for _, v in scored]],
            "distances": [[d for d, _ in scored]],
        }


class PersistentClient:
    def __init__(self, path):
        self.path = path

    def get_or_create_collection(self, name, embedding_function=None, configuration=None):
        return _Collection(self.path, name, embedding_function, configuration)
