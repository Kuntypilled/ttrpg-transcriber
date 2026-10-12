# Alias candidates: Report fixture session 3

- transcript: `/TMP/vault/0.2_campaigns/demo/08_transcripts/S03/transcript.md`
- words scanned: 420
- similarity threshold: 0.6   minimum frequency for section B: 1
- existing rules: 2 alias, 1 review_only

Nothing here has been applied. This is a report. Copy what you confirm into
`canon_aliases.json` by hand.

**A mangling that is also a real English word belongs in `review_only`, never in**
**`aliases`.** Auto-replacing "complete" or "leave" would quietly wreck a transcript.

## A1. Near-misses that are NOT English words

Candidates for `aliases`. Safe to auto-replace, because nothing else in the
language spells them that way.

| canon | heard as | times | similarity |
|---|---|---|---|
| Vashti Corrow | Vashtie Corrow | 8 | 0.96 |
| Vashti Corrow | Vashti Corow | 8 | 0.96 |
| Marrow Hall | Marow Hall | 6 | 0.95 |
| Emberfall | Emberfal | 8 | 0.94 |
| Thornwake | Thornwak | 8 | 0.94 |
| Kestrel Vane | Kestral Vane | 4 | 0.92 |
| Cinder Road | Sinder Road | 5 | 0.91 |
| Wern | Wernn | 6 | 0.89 |
| Wern | Wirn | 6 | 0.75 |
| Bram Ollander | and Ostrander | 5 | 0.62 |

## A2. Near-misses that ARE English words

Candidates for `review_only` only. Expect noise here: short names sit close to
common words, so `Wern`/`were` and `Dunwell`/`well` will always show up. That is
harmless, because review_only is never applied automatically. Raise --threshold
to quiet it down.

| canon | heard as | times | similarity |
|---|---|---|---|
| Nell | well | 12 | 0.75 |
| Wern | were | 7 | 0.75 |
| Wern | where | 4 | 0.67 |
| Cinder Road | the road | 6 | 0.63 |

## B. Unrecognised proper nouns

Capitalised, not in the dictionary, not already canon. New names, or manglings
too far from the canon spelling for section A to see.

| token | times |
|---|---|
| Zorblax | 18 |
| Quillhaven | 12 |
| Brannoch | 10 |
| Tholwyn | 10 |
| Vashti | 8 |
| Corow | 8 |
| Vashtie | 8 |
| Corrow | 8 |
| Marro | 6 |
| Marow | 6 |
| Sinder | 5 |
| Ostrander | 5 |
| Kestrel | 4 |
| Kestral | 4 |
| Vane | 4 |

## Ready to paste

Merge these into the existing objects. Do not replace them.

`aliases` (safe, auto-replaced at the chunker):

```json
{
  "Bram Ollander": [
    "and Ostrander"
  ],
  "Cinder Road": [
    "Sinder Road"
  ],
  "Emberfall": [
    "Emberfal"
  ],
  "Kestrel Vane": [
    "Kestral Vane"
  ],
  "Marrow Hall": [
    "Marow Hall"
  ],
  "Thornwake": [
    "Thornwak"
  ],
  "Vashti Corrow": [
    "Vashti Corow",
    "Vashtie Corrow"
  ],
  "Wern": [
    "Wernn",
    "Wirn"
  ]
}
```

`review_only` (NEVER auto-replaced, surfaced in the recap pass):

```json
{
  "Cinder Road": [
    "the road"
  ],
  "Nell": [
    "well"
  ],
  "Wern": [
    "were",
    "where"
  ]
}
```

