# History rewrite

On 2026-10-12 the git history of this repository was rewritten. This page says what
changed and why, so that anyone who finds an old clone or a dangling link knows what
happened.

## Why

Several files the repository started with, and one hand-over document added while the
terminal program was being written, contained names from a real tabletop campaign:
character names, aliases, a campaign id, an NPC title and the title of a note. They were
in early versions of the README, the example files and two of the Python scripts. They were
removed from the current files first, and then from every earlier commit, so that no
version of the repository contains them. One more name, of a player who will join a future
session, was removed the same way.

## What changed

- **Every commit's contents**, by a scripted text substitution applied to every text file
  in every commit: each real name became an invented one (the stand-ins used in the
  examples, `Emberfall`, `Vashti`, `Kestrel Vane` and so on). Nothing else in the files
  was edited by hand.
- **Fixtures that depended on those strings** were recomputed inside each commit, so the
  history stays testable: the ratios in `difflib_pairs.json` come from Python's `difflib`
  on the renamed strings, and the alias-report goldens were regenerated from the renamed
  inputs by their own `generate.sh`. The test suite passes at the commits it was run on
  (the Index milestone, the pipeline milestone, the cleanup milestone and the commit that
  prepared the merge).
- **Commit hashes.** Every hash changed, including those of commits whose content did not,
  because a hash covers the history before it. Hashes quoted in older files (for example
  `98aa387` in the oracle README as first written) refer to the old history. The commit
  that is now `3f6dcca` was `98aa387`.
- **One commit was dropped**: a late commit that only swapped a name was empty once the
  names were gone from the commits before it.

## What did not change

Authors, committer names and dates, commit messages (none contained a real name), the
order of commits, and the content of the current tree apart from the one player name:
the files at the tip are identical to the files at the tip before the rewrite, except for
that name.

## If you have an old clone

Its history no longer shares commits with this one. Re-clone instead of pulling. Forks and
any copy made before 2026-10-12 still hold the old history; this repository cannot change
those.
