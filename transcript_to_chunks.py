#!/usr/bin/env python3
"""
Stage 2 of the evidence layer, for any campaign.
Parse a whisper.cpp/Craig merged transcript.md -> apply SAFE name fixes from the
campaign's canon_aliases.json -> window into ~280-word conversational chunks with
metadata -> write JSONL.

Usage:
  transcript_to_chunks.py --campaign my_campaign --session 13
  transcript_to_chunks.py --campaign my_campaign --session 13 -o -        # stdout
  transcript_to_chunks.py --campaign my_campaign --session 1 --date 2026-08-05
  transcript_to_chunks.py --campaign my_campaign --session 1 --report    # see below

Paths default to the campaign's vault folder:
  in   0.2_campaigns/<c>/08_transcripts/S<nn>/transcript.md
  out  0.2_campaigns/<c>/08_transcripts/chunks/s<nn>_chunks.jsonl

Speaker labels are read from the transcript. Character identity is resolved via
the campaign's speaker_map (explicit, not inferred), so a change in Craig's
filename convention can't silently break the metadata. Add a row per voice you
record -- in canon_aliases.json, not here.

Only `aliases` are applied. `review_only` is never auto-replaced: those manglings
collide with real English words ("borrow" for Corrow, "vain" for Vane,
"seated" for seeded) and substituting them would quietly destroy a transcript.

--report runs a read-only alias-candidate pass instead of chunking. It writes a
markdown report of likely manglings and unrecognised proper nouns, split into
"safe to auto-replace" and "review only", so a new campaign can seed its
review_only list BEFORE the first ingest rather than discovering the collisions
afterwards. It never edits canon_aliases.json.
"""
import argparse
import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import campaign as campaign_mod


# ── Parsing ───────────────────────────────────────────────────────────────────

UTT = re.compile(
    r'\*\*\[(\d{2}):(\d{2}):(\d{2})\]\s+([^:]+):\*\*\s*\n(.*?)(?=\n\*\*\[|\Z)',
    re.DOTALL)


def make_clean(subs):
    def clean(text):
        for pat, canon in subs:
            text = pat.sub(canon, text)
        return text
    return clean


def make_resolve(speaker_map):
    def resolve(label):
        low = label.lower()
        role = "GM" if ("gm" in low or "-gm-" in low) else "Player"
        for key, player, char in speaker_map:
            if key in low:
                return ("GM" if char is None else role), player, char
        return role, None, None
    return resolve


def parse(raw, clean, resolve):
    out = []
    for m in UTT.finditer(raw):
        h, mn, s, spk, text = m.groups()
        role, player, char = resolve(spk.strip())
        out.append(dict(ts=f"{h}:{mn}:{s}", role=role, player=player, char=char,
                        label=char or "GM", text=clean(text.strip())))
    return [u for u in out if u["text"]]


def window(utts, target_words=280, overlap=1):
    chunks, i = [], 0
    while i < len(utts):
        j, w, block = i, 0, []
        while j < len(utts) and w < target_words:
            block.append(utts[j]); w += len(utts[j]["text"].split()); j += 1
        chars = sorted({u["char"] for u in block if u["char"]})
        body = "\n".join(f"[{u['ts']}] {u['label']}: {u['text']}" for u in block)
        chunks.append(dict(block=block, characters=chars,
                           has_gm=any(u["role"] == "GM" for u in block),
                           start=block[0]["ts"], end=block[-1]["ts"],
                           words=w, n_utt=len(block), text=body))
        if j >= len(utts): break
        i = max(j - overlap, i + 1)
    return chunks


# ── Chunking run ──────────────────────────────────────────────────────────────

def run_chunk(camp, session, date, transcript_path, out_stream):
    subs = camp.alias_subs()
    utts = parse(open(transcript_path, encoding="utf-8").read(),
                 make_clean(subs), make_resolve(camp.speaker_map))
    chunks = window(utts)
    pcs = camp.pcs
    for k, c in enumerate(chunks):
        meta = {"session": session, "date": date, "start": c["start"], "end": c["end"],
                "has_gm": c["has_gm"], "n_utt": c["n_utt"],
                "characters": ",".join(c["characters"])}
        for name in pcs:
            meta[f"has_{name}"] = name in c["characters"]
        out_stream.write(json.dumps({"id": f"s{session:02d}_c{k:03d}",
                                     "document": c["text"], "metadata": meta}) + "\n")
    sys.stderr.write(f"{len(chunks)} chunks from {len(utts)} utterances "
                     f"(session {session}); {len(subs)} alias rules applied\n")
    return len(chunks)


# ── Alias-candidate report ────────────────────────────────────────────────────
# Read-only. Seeds a new campaign's review_only before the first ingest.
#
# Two signals, because they catch different failures:
#   A. Near-misses of names already in canon. Catches "Vashtee" for Vashti.
#   B. Frequent capitalised tokens that are neither canon nor dictionary words.
#      Catches manglings too far from the canon spelling for a ratio to see.
#
# A candidate whose every word is a real English word goes to review_only, never
# to aliases. That is the whole point of the split.

WORDLISTS = ("/usr/share/dict/words",
             "/usr/share/dict/american-english",
             "/usr/share/dict/british-english")

TOKEN = re.compile(r"[A-Za-z][A-Za-z'’\-]*")
POSSESSIVE = re.compile(r"['’]s$")


def base_form(gram):
    """Drop a trailing possessive.

    "katrice's" is already handled by the "katrice" alias, because the \\b before
    the apostrophe lets the substitution keep the 's. Proposing the possessive as
    its own alias would replace "katrice's" with "Catrice" and eat the possessive.
    """
    return POSSESSIVE.sub("", gram)


def load_dictionary():
    words = set()
    for path in WORDLISTS:
        if os.path.isfile(path):
            try:
                with open(path, encoding="utf-8", errors="ignore") as f:
                    words.update(w.strip().lower() for w in f if w.strip())
            except OSError:
                continue
            break
    return words


def _speaker_labels(raw):
    return {m.group(4).strip() for m in UTT.finditer(raw)}


def build_report(camp, session, transcript_path, threshold, min_freq):
    from collections import Counter
    from difflib import SequenceMatcher

    raw = open(transcript_path, encoding="utf-8").read()
    body = "\n".join(m.group(5) for m in UTT.finditer(raw)) or raw
    words = TOKEN.findall(body)
    dictionary = load_dictionary()

    canon_terms, seen = [], set()
    for term in list(camp.names) + camp.pcs + list(camp.aliases) + list(camp.review_only):
        t = str(term).replace("_", " ").strip()
        if len(t) >= 3 and t.lower() not in seen:
            seen.add(t.lower())
            canon_terms.append(t)

    # Everything already accounted for: canon spellings, confirmed manglings,
    # nicknames and the speaker labels themselves.
    known = set(seen)
    for table in (camp.aliases, camp.review_only):
        for variants in table.values():
            known.update(v.lower() for v in variants)
    for variants in camp.data.get("pc_nicknames", {}).values():
        if isinstance(variants, list):
            known.update(v.lower() for v in variants)
    known.update(lbl.lower().replace("_", " ") for lbl in _speaker_labels(raw))

    # n-gram pools, one per canon term length
    max_n = max((len(t.split()) for t in canon_terms), default=1)
    max_n = min(max_n, 3)
    pools = {}
    for n in range(1, max_n + 1):
        counter = Counter()
        for i in range(len(words) - n + 1):
            gram = base_form(" ".join(words[i:i + n]))
            if len(gram) >= 4:
                counter[gram] += 1
        pools[n] = counter

    # -- A. near-misses of known canon --
    near = {}     # canon -> {variant: count}
    for term in canon_terms:
        n = min(len(term.split()), max_n)
        tl = term.lower()
        for gram, count in pools[n].items():
            gl = gram.lower()
            if gl == tl or gl in known:
                continue
            sm = SequenceMatcher(None, tl, gl)
            if sm.real_quick_ratio() < threshold or sm.quick_ratio() < threshold:
                continue
            ratio = sm.ratio()
            if ratio >= threshold:
                prev = near.setdefault(term, {}).get(gram)
                if prev is None or count > prev[0]:
                    near[term][gram] = (count, ratio)

    # Keep each variant only against its single best canon term.
    best = {}
    for term, variants in near.items():
        for gram, (count, ratio) in variants.items():
            if gram not in best or ratio > best[gram][2]:
                best[gram] = (term, count, ratio)
    near = {}
    for gram, (term, count, ratio) in best.items():
        near.setdefault(term, []).append((gram, count, ratio))
    for term in near:
        near[term].sort(key=lambda r: (-r[2], -r[1]))

    def is_english(gram):
        return all(w.lower().strip("'’-") in dictionary for w in gram.split()) if dictionary else False

    # -- B. unrecognised proper-noun-ish tokens --
    caps = Counter()
    for w in words:
        w = base_form(w)
        if not w[:1].isupper() or len(w) < 4:
            continue
        wl = w.lower()
        if wl in dictionary or wl in known or wl in {g.lower() for g in best}:
            continue
        caps[w] += 1
    unknown_caps = [(w, c) for w, c in caps.most_common() if c >= min_freq]

    return near, unknown_caps, is_english, len(words)


def render_report(camp, session, transcript_path, near, unknown_caps, is_english,
                  n_words, threshold, min_freq):
    L = []
    A = L.append
    A(f"# Alias candidates: {camp.label} session {session}")
    A("")
    A(f"- transcript: `{transcript_path}`")
    A(f"- words scanned: {n_words}")
    A(f"- similarity threshold: {threshold}   minimum frequency for section B: {min_freq}")
    A(f"- existing rules: {sum(len(v) for v in camp.aliases.values())} alias, "
      f"{sum(len(v) for v in camp.review_only.values())} review_only")
    A("")
    A("Nothing here has been applied. This is a report. Copy what you confirm into")
    A("`canon_aliases.json` by hand.")
    A("")
    A("**A mangling that is also a real English word belongs in `review_only`, never in**")
    A("**`aliases`.** Auto-replacing \"complete\" or \"leave\" would quietly wreck a transcript.")
    A("")

    safe, review = {}, {}
    safe_rows, review_rows = [], []
    for term in sorted(near):
        for gram, count, ratio in near[term]:
            if is_english(gram):
                review.setdefault(term, []).append(gram)
                review_rows.append((term, gram, count, ratio))
            else:
                safe.setdefault(term, []).append(gram)
                safe_rows.append((term, gram, count, ratio))

    def table(rows):
        if not rows:
            A("None above threshold.")
            return
        A("| canon | heard as | times | similarity |")
        A("|---|---|---|---|")
        for term, gram, count, ratio in sorted(rows, key=lambda r: (-r[3], -r[2])):
            A(f"| {term} | {gram} | {count} | {ratio:.2f} |")

    A("## A1. Near-misses that are NOT English words")
    A("")
    A("Candidates for `aliases`. Safe to auto-replace, because nothing else in the")
    A("language spells them that way.")
    A("")
    table(safe_rows)
    A("")

    A("## A2. Near-misses that ARE English words")
    A("")
    A("Candidates for `review_only` only. Expect noise here: short names sit close to")
    A("common words, so `Wren`/`were` and `Dimwell`/`well` will always show up. That is")
    A("harmless, because review_only is never applied automatically. Raise --threshold")
    A("to quiet it down.")
    A("")
    table(review_rows)
    A("")

    A("## B. Unrecognised proper nouns")
    A("")
    A("Capitalised, not in the dictionary, not already canon. New names, or manglings")
    A("too far from the canon spelling for section A to see.")
    A("")
    if not unknown_caps:
        A("None above the frequency floor.")
    else:
        A("| token | times |")
        A("|---|---|")
        for w, c in unknown_caps[:60]:
            A(f"| {w} | {c} |")
        if len(unknown_caps) > 60:
            A("")
            A(f"({len(unknown_caps) - 60} more below the cut.)")
    A("")

    A("## Ready to paste")
    A("")
    A("Merge these into the existing objects. Do not replace them.")
    A("")
    A("`aliases` (safe, auto-replaced at the chunker):")
    A("")
    A("```json")
    A(json.dumps({k: sorted(set(v)) for k, v in sorted(safe.items())},
                 indent=2, ensure_ascii=False) if safe else "{}")
    A("```")
    A("")
    A("`review_only` (NEVER auto-replaced, surfaced in the recap pass):")
    A("")
    A("```json")
    A(json.dumps({k: sorted(set(v)) for k, v in sorted(review.items())},
                 indent=2, ensure_ascii=False) if review else "{}")
    A("```")
    A("")
    return "\n".join(L) + "\n"


# ── CLI ───────────────────────────────────────────────────────────────────────

def main(argv=None):
    ap = argparse.ArgumentParser(
        description="Chunk a session transcript for one campaign, or report alias candidates.")
    campaign_mod.add_campaign_argument(ap)
    ap.add_argument("--session", type=int, required=True)
    ap.add_argument("--date", help="session date YYYY-MM-DD (default: today)")
    ap.add_argument("--transcript", help="override the transcript.md path")
    ap.add_argument("-o", "--out", help="output path, or - for stdout "
                                        "(default: the campaign's chunks folder)")
    ap.add_argument("--report", action="store_true",
                    help="read-only alias-candidate pass instead of chunking")
    ap.add_argument("--threshold", type=float, default=0.70,
                    help="report: similarity floor for section A (default 0.70)")
    ap.add_argument("--min-freq", type=int, default=2,
                    help="report: frequency floor for section B (default 2)")
    a = ap.parse_args(argv)

    try:
        camp = campaign_mod.load(a.campaign)
    except campaign_mod.CampaignError as exc:
        sys.stderr.write(f"ERROR: {exc}\n")
        return 2

    transcript_path = a.transcript or camp.transcript_path(a.session)
    if not os.path.isfile(transcript_path):
        sys.stderr.write(f"ERROR: transcript not found: {transcript_path}\n")
        return 2

    if a.report:
        near, caps, is_english, n_words = build_report(
            camp, a.session, transcript_path, a.threshold, a.min_freq)
        text = render_report(camp, a.session, transcript_path, near, caps,
                             is_english, n_words, a.threshold, a.min_freq)
        out = a.out or os.path.join(camp.session_dir(a.session), "alias_candidates.md")
        if out == "-":
            sys.stdout.write(text)
        else:
            os.makedirs(os.path.dirname(out), exist_ok=True)
            with open(out, "w", encoding="utf-8") as f:
                f.write(text)
            sys.stderr.write(f"Alias candidate report written to {out}\n")
        return 0

    if not camp.speaker_map:
        sys.stderr.write(f"ERROR: {a.campaign} has no speaker_map and none could be "
                         f"derived. Add one to {camp.aliases_path}.\n")
        return 2

    date = a.date
    if not date:
        import datetime
        date = datetime.date.today().isoformat()

    out = a.out or camp.chunks_path(a.session)
    if out == "-":
        run_chunk(camp, a.session, date, transcript_path, sys.stdout)
    else:
        os.makedirs(os.path.dirname(out), exist_ok=True)
        with open(out, "w", encoding="utf-8") as f:
            run_chunk(camp, a.session, date, transcript_path, f)
        sys.stderr.write(f"Wrote {out}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
