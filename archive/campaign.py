#!/usr/bin/env python3
"""
Campaign registry and path resolution for the session transcription pipeline.

A campaign is any folder under a vault's 0.2_campaigns/ that contains a
canon_aliases.json. That file is the whole registry. There is no second config
to keep in sync, so adding a campaign means creating one folder with one JSON
file and editing no Python.

Two vault layouts are supported:

  multi-vault   $GM_VAULTS_ROOT/<world>/0.2_campaigns/<c>/    one vault per world
                campaign id is "<world>/<c>", e.g. "emberfall/main"
  single vault  $GM_VAULT/0.2_campaigns/<c>/                  everything in one vault
                campaign id is "<c>"

If a vaults root is configured it wins. In multi-vault mode the world is part of
the campaign id, so two worlds can each have a campaign called "main" without
sharing audio, Chroma or work directories.

Everything campaign-specific lives in that JSON:

  campaign      slug/label for display (optional; folder name is the identity)
  world         which world the campaign runs in (optional, informational)
  system        daggerheart / dnd-5e / ... (optional, shown in the GUI)
  seed_prompt   fed to whisper.cpp --prompt
  aliases       SAFE substitutions, auto-applied at the chunker
  review_only   manglings that collide with real English words, NEVER auto-applied
  names         canon proper nouns, used by the alias-candidate report
  speaker_map   ordered [{match, player, character}], first match wins
  players       {character: player}; its key order defines the has_<PC> metadata
  discord_names {craig_username: {player, character, role}}
  audio_root    optional per-campaign override for where Craig audio lives

Paths this module owns:

  transcripts   <vault>/<campaigns>/<c>/<transcripts>/S<nn>/
  chunks        <vault>/<campaigns>/<c>/<transcripts>/chunks/s<nn>_chunks.jsonl
  audio         $GM_AUDIO_ROOT/<id>/Session <n>         (outside the vault: large + binary)
  chroma        $GM_CHROMA_ROOT/<id>/                   (outside the vault: binary + unsyncable)
  work          $GM_WORK_ROOT/<id>/S<nn>/               (outside the vault: whisper intermediates)

  <vault> is the world's vault in multi-vault mode; <id> is the campaign id above.

  <campaigns> defaults to 0.2_campaigns and <transcripts> to 08_transcripts.
  Both are overridable, so the tool does not require one person's vault layout.

Audio, the Chroma store and the whisper working files all stay out of the vault.
The Chroma store in particular is a SQLite file plus an HNSW index: Obsidian Sync
would push megabytes per ingest, two devices writing it has no locking, and it is
fully regenerable from the JSONL, which is text and does live in the vault.

CLI (used by transcribe.sh so path rules are not duplicated in bash):

  python3 campaign.py list
  python3 campaign.py paths --campaign my_campaign --session 13
  python3 campaign.py paths --campaign emberfall/main --session 13     (multi-vault)
"""

import glob
import json
import os
import re
import sys

class CampaignError(Exception):
    pass


# ── Roots ─────────────────────────────────────────────────────────────────────
# No path in this file is tied to one machine. Every root resolves in this order:
#
#   1. an environment variable
#   2. the config file, $XDG_CONFIG_HOME/ttrpg-transcriber/config.json
#   3. a built-in default, where a correct one exists
#
# Where the vaults live is set by $GM_VAULTS_ROOT / "vaults_root" (a directory
# holding one vault per world) or, for a single vault, $GM_VAULT / "vault".
# If both are configured the vaults root wins.
#
# Neither has a built-in default, deliberately. There is no right guess
# for where a person keeps their vaults, and a wrong guess fails quietly: the
# campaign glob matches nothing, the dropdown comes up empty, and nothing says
# why. An unconfigured vault raises with instructions instead.
#
# The chroma and work defaults land on ~/.local/share/ttrpg_memory and
# ~/.cache/ttrpg_transcribe on any system where XDG is unset, which is where
# they already were, so an existing store needs no migration.

CONFIG_ENV     = "GM_TRANSCRIBER_CONFIG"
CONFIG_RELPATH = os.path.join("ttrpg-transcriber", "config.json")

WHISPER_DEVICE_ENV = "GM_WHISPER_DEVICE"

DEFAULT_CAMPAIGNS_SUBDIR   = "0.2_campaigns"
DEFAULT_TRANSCRIPTS_SUBDIR = "08_transcripts"
ALIASES_FILENAME           = "canon_aliases.json"

UNCONFIGURED = (
    "No vault is configured, so there are no campaigns to find.\n"
    "\n"
    "One vault per world (a directory containing <world>/0.2_campaigns/...):\n"
    "    export GM_VAULTS_ROOT=/path/to/vaults\n"
    '    or put {"vaults_root": "/path/to/vaults"} in the config file\n'
    "\n"
    "One vault for everything:\n"
    "    export GM_VAULT=/path/to/your/vault\n"
    '    or put {"vault": "/path/to/your/vault"} in the config file\n'
    "\n"
    "Then run:  python3 campaign.py doctor"
)


def _xdg(name, fallback):
    val = os.environ.get(name)
    if val and os.path.isabs(val):
        return val
    return os.path.expanduser(fallback)


def xdg_config_home():
    return _xdg("XDG_CONFIG_HOME", "~/.config")


def xdg_data_home():
    return _xdg("XDG_DATA_HOME", "~/.local/share")


def xdg_cache_home():
    return _xdg("XDG_CACHE_HOME", "~/.cache")


def xdg_state_home():
    return _xdg("XDG_STATE_HOME", "~/.local/state")


def _abspath(value):
    return os.path.abspath(os.path.expanduser(str(value)))


def config_path():
    override = os.environ.get(CONFIG_ENV)
    if override:
        return _abspath(override)
    return os.path.join(xdg_config_home(), CONFIG_RELPATH)


_config_cache = None


def config(reload=False):
    """Contents of config.json, or {} when there is no config file."""
    global _config_cache
    if _config_cache is None or reload:
        path = config_path()
        try:
            with open(path, encoding="utf-8") as f:
                data = json.load(f)
        except FileNotFoundError:
            data = {}
        except json.JSONDecodeError as exc:
            raise CampaignError(f"{path} is not valid JSON: {exc}") from exc
        _config_cache = data if isinstance(data, dict) else {}
    return _config_cache


def _resolve(config_key, *env_names, default=None):
    """(absolute path, where it came from). (None, None) if nothing supplies it."""
    for name in env_names:
        val = os.environ.get(name)
        if val:
            return _abspath(val), f"${name}"
    val = config().get(config_key)
    if val:
        return _abspath(val), config_path()
    if default is None:
        return None, None
    return _abspath(default), "built-in default"


def _setting(config_key, env_name, default):
    return os.environ.get(env_name) or config().get(config_key) or default


def vaults_root_source():
    """(path, source) of the directory holding one vault per world, or (None, None)."""
    return _resolve("vaults_root", "GM_VAULTS_ROOT")


def multi_vault():
    return vaults_root_source()[0] is not None


def vault_root_source():
    """Single-vault root. Raises when unconfigured; use campaigns_dir_for in multi-vault mode."""
    path, src = _resolve("vault", "GM_VAULT", "VAULT")
    if not path:
        raise CampaignError(UNCONFIGURED)
    return path, src


def vault_root():
    return vault_root_source()[0]


def split_slug(slug):
    """(world, campaign) for a campaign id. world is None in single-vault mode.

    In multi-vault mode the id must be exactly "<world>/<campaign>". Anything
    else, including '..' segments, is rejected rather than resolved.
    """
    parts = str(slug).split("/")
    if multi_vault():
        if len(parts) != 2:
            raise CampaignError(
                f"'{slug}' is not a campaign id. Multi-vault mode expects "
                f"<world>/<campaign>, e.g. emberfall/main.\n"
                f"Run: python3 campaign.py list")
        world, camp = parts
    else:
        if len(parts) != 1:
            raise CampaignError(
                f"'{slug}' is not a campaign id. Single-vault mode expects a bare "
                f"campaign folder name. To use <world>/<campaign> ids, set "
                f"GM_VAULTS_ROOT.")
        world, camp = None, parts[0]
    for part in (world, camp):
        if part is not None and part in ("", ".", ".."):
            raise CampaignError(f"'{slug}' is not a valid campaign id.")
    return world, camp


def campaigns_subdir():
    return _setting("campaigns_subdir", "GM_CAMPAIGNS_SUBDIR", DEFAULT_CAMPAIGNS_SUBDIR)


def transcripts_subdir():
    return _setting("transcripts_subdir", "GM_TRANSCRIPTS_SUBDIR", DEFAULT_TRANSCRIPTS_SUBDIR)


def world_vault(world):
    """The vault directory for one world (multi-vault mode)."""
    root, _ = vaults_root_source()
    if root is None:
        raise CampaignError(UNCONFIGURED)
    return os.path.join(root, world)


def campaigns_dir(world=None):
    """Folder holding the campaigns. `world` is required in multi-vault mode."""
    if multi_vault():
        if world is None:
            raise CampaignError("campaigns_dir() needs a world in multi-vault mode")
        return os.path.join(world_vault(world), campaigns_subdir())
    return os.path.join(vault_root(), campaigns_subdir())


def audio_root_source():
    return _resolve("audio_root", "GM_AUDIO_ROOT",
                    default=os.path.join(xdg_data_home(), "ttrpg_audio"))


def chroma_root_source():
    return _resolve("chroma_root", "GM_CHROMA_ROOT",
                    default=os.path.join(xdg_data_home(), "ttrpg_memory"))


def work_root_source():
    return _resolve("work_root", "GM_WORK_ROOT",
                    default=os.path.join(xdg_cache_home(), "ttrpg_transcribe"))


def whisper_device_source():
    """(device index, where it came from), or (None, None) to let whisper-cli choose.

    This is whisper-cli's -dev N: the ggml device index, which is the number
    ggml prints at startup ("ggml_vulkan: 1 = NVIDIA ..."). It matters on a
    machine with more than one GPU, where index 0 is not always the fast one.
    """
    for src, val in ((f"${WHISPER_DEVICE_ENV}", os.environ.get(WHISPER_DEVICE_ENV)),
                     (config_path(), config().get("whisper_device"))):
        if val is None or val == "":
            continue
        try:
            idx = int(str(val).strip())
        except ValueError:
            idx = -1
        if idx < 0:
            raise CampaignError(
                f"whisper_device from {src} is {val!r}; it must be a whole number, "
                f"0 or higher (the device index whisper-cli prints at startup).")
        return idx, src
    return None, None


def audio_root():
    return audio_root_source()[0]


def chroma_root():
    return chroma_root_source()[0]


def work_root():
    return work_root_source()[0]


def setup_hint():
    """One line for the GUI when no campaign loaded: what is wrong, or where it looked."""
    if multi_vault():
        return (f"No campaigns found under {vaults_root_source()[0]}/<world>/"
                f"{campaigns_subdir()}/")
    try:
        return f"No campaigns found under {campaigns_dir()}"
    except CampaignError:
        return ("No vault configured. Set GM_VAULTS_ROOT (or GM_VAULT), "
                "then run: python3 campaign.py doctor")


# ── Naming ────────────────────────────────────────────────────────────────────
# Session folders are S<nn>, chunk files s<nn>_chunks.jsonl, zero-padded below 10.
# This matches transcript folders filed by hand before the tool existed.

def session_folder(session):
    return f"S{int(session):02d}"


def chunks_filename(session):
    return f"s{int(session):02d}_chunks.jsonl"


def normalize_character(name):
    """Character identity used in metadata and speaker labels.

    Spaces become underscores so 'Kestrel Vane' in players matches the
    'Kestrel_Vane' that transcribe.sh already writes into transcript.md,
    and so has_<PC> metadata keys stay single-token.
    """
    if name is None:
        return None
    return re.sub(r"\s+", "_", str(name).strip())


# ── Campaign ──────────────────────────────────────────────────────────────────

class Campaign:
    """One campaign folder, backed by its canon_aliases.json."""

    def __init__(self, slug, root, data):
        self.slug = slug
        self.root = root
        self.data = data

    # -- discovery --

    @classmethod
    def load(cls, slug):
        world, name = split_slug(slug)
        root = os.path.join(campaigns_dir(world), name)
        path = os.path.join(root, ALIASES_FILENAME)
        if not os.path.isfile(path):
            raise CampaignError(
                f"No campaign '{slug}': {path} does not exist.\n"
                f"Known campaigns: {', '.join(s for s, _ in list_campaigns()) or '(none)'}")
        with open(path, encoding="utf-8") as f:
            try:
                data = json.load(f)
            except json.JSONDecodeError as exc:
                raise CampaignError(f"{path} is not valid JSON: {exc}") from exc
        return cls(slug, root, data)

    # -- identity --

    @property
    def aliases_path(self):
        return os.path.join(self.root, ALIASES_FILENAME)

    @property
    def label(self):
        return self.data.get("display_name") or self.data.get("campaign") or self.slug

    @property
    def system(self):
        return self.data.get("system") or ""

    @property
    def world(self):
        """World name: the vault folder in multi-vault mode, else the JSON field."""
        if "/" in self.slug:
            return self.slug.split("/", 1)[0]
        return self.data.get("world") or ""

    @property
    def menu_label(self):
        return f"{self.label}  —  {self.system}" if self.system else self.label

    # -- paths --

    @property
    def transcripts_dir(self):
        return os.path.join(self.root, transcripts_subdir())

    def session_dir(self, session):
        return os.path.join(self.transcripts_dir, session_folder(session))

    @property
    def chunks_dir(self):
        return os.path.join(self.transcripts_dir, "chunks")

    def chunks_path(self, session):
        return os.path.join(self.chunks_dir, chunks_filename(session))

    def transcript_path(self, session):
        return os.path.join(self.session_dir(session), "transcript.md")

    @property
    def audio_dir_base(self):
        override = self.data.get("audio_root")
        if override:
            return os.path.abspath(os.path.expanduser(override))
        return os.path.join(audio_root(), self.slug)

    def audio_dir(self, session):
        return os.path.join(self.audio_dir_base, f"Session {int(session)}")

    @property
    def db_path(self):
        return os.path.join(chroma_root(), self.slug)

    def work_dir(self, session):
        return os.path.join(work_root(), self.slug, session_folder(session))

    # -- campaign data --

    @property
    def seed_prompt(self):
        return self.data.get("seed_prompt", "")

    @property
    def speaker_map(self):
        """Ordered [(match, player, character)].

        Matching is case-insensitive substring against the transcript's speaker
        label and ORDER MATTERS: the first match wins. Falls back to deriving a
        map from players/discord_names so a campaign whose first recording has
        not happened yet still resolves its own characters.
        """
        rows = self.data.get("speaker_map")
        if isinstance(rows, list) and rows:
            out = []
            for row in rows:
                if not isinstance(row, dict) or "match" not in row:
                    continue
                out.append((str(row["match"]).lower(),
                            row.get("player"),
                            normalize_character(row.get("character"))))
            if out:
                return out
        return self._derived_speaker_map()

    def _derived_speaker_map(self):
        out, seen = [], set()
        for char, player in self.players.items():
            for key in (char.lower(), (player or "").lower()):
                if key and key not in seen:
                    seen.add(key)
                    out.append((key, player, char))
        for info in self.data.get("discord_names", {}).values():
            if not isinstance(info, dict):
                continue
            if info.get("role") == "GM":
                for key in ("gm", (info.get("player") or "").lower()):
                    if key and key not in seen:
                        seen.add(key)
                        out.append((key, info.get("player"), None))
        if "gm" not in seen:
            out.append(("gm", None, None))
        return out

    @property
    def players(self):
        """{normalized character: player}. Key ORDER defines has_<PC> metadata order."""
        raw = self.data.get("players")
        out = {}
        if isinstance(raw, dict):
            for char, player in raw.items():
                if char.startswith("_"):
                    continue
                out[normalize_character(char)] = player
        if out:
            return out
        # Fall back to the characters named in speaker_map, in first-seen order.
        rows = self.data.get("speaker_map")
        if isinstance(rows, list):
            for row in rows:
                if not isinstance(row, dict):
                    continue
                char = normalize_character(row.get("character"))
                if char and char not in out:
                    out[char] = row.get("player")
        return out

    @property
    def pcs(self):
        return list(self.players.keys())

    @property
    def discord_names(self):
        return {k: v for k, v in self.data.get("discord_names", {}).items()
                if not k.startswith("_")}

    @property
    def names(self):
        return [n for n in self.data.get("names", []) if isinstance(n, str)]

    def _variant_map(self, key):
        out = {}
        for canon, variants in self.data.get(key, {}).items():
            if canon.startswith("_") or not isinstance(variants, list):
                continue
            out[canon] = [v for v in variants if isinstance(v, str)]
        return out

    @property
    def aliases(self):
        """SAFE substitutions. Auto-applied to the transcript by the chunker."""
        return self._variant_map("aliases")

    @property
    def review_only(self):
        """Manglings that collide with real English words. NEVER auto-applied.

        'borrow' for Corrow, 'vain' for Vane, 'narrow' for Marrow. Replacing
        these automatically would quietly destroy a transcript, so they are only
        ever surfaced for a human call in the recap pass.
        """
        return self._variant_map("review_only")

    @property
    def has_review_only(self):
        return bool(self.review_only)

    def alias_subs(self):
        """Compiled (pattern, canon) pairs in file order. aliases only, never review_only."""
        subs = []
        for canon, variants in self.aliases.items():
            for v in variants:
                subs.append((re.compile(rf"\b{re.escape(v)}\b", re.IGNORECASE), canon))
        return subs


# ── Surgical JSON append ──────────────────────────────────────────────────────
# canon_aliases.json is hand-maintained and its key order, column alignment and
# _comment fields are load-bearing documentation. json.dump would reformat the
# whole file, so new entries are spliced in as text and the result is verified to
# parse and to leave every pre-existing key untouched.

def _object_body_span(text, key):
    """(index of the object's '{', index of its matching '}') for a top-level key."""
    m = re.search(rf'"{re.escape(key)}"\s*:\s*\{{', text)
    if not m:
        raise CampaignError(f'no "{key}" object found to append to')
    open_idx = m.end() - 1
    depth, i, in_str, esc = 0, open_idx, False, False
    while i < len(text):
        ch = text[i]
        if in_str:
            if esc:
                esc = False
            elif ch == "\\":
                esc = True
            elif ch == '"':
                in_str = False
        elif ch == '"':
            in_str = True
        elif ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                return open_idx, i
        i += 1
    raise CampaignError(f'"{key}" object is not closed')


def append_object_entries(path, key, entries, indent="    "):
    """Splice entries into the JSON object at `key`, preserving the rest verbatim.

    Returns the keys actually added. Existing keys are left alone, never rewritten.
    """
    with open(path, encoding="utf-8") as f:
        text = f.read()
    original = json.loads(text)

    existing = original.get(key)
    if not isinstance(existing, dict):
        raise CampaignError(f'"{key}" is not an object in {path}')
    todo = {k: v for k, v in entries.items() if k not in existing}
    if not todo:
        return []

    open_idx, close_idx = _object_body_span(text, key)
    body = text[open_idx + 1:close_idx]
    content = body.rstrip()
    tail = body[len(content):] or "\n"
    if content and not content.endswith(","):
        content += ","
    for k, v in todo.items():
        content += f"\n{indent}{json.dumps(k, ensure_ascii=False)}: {json.dumps(v, ensure_ascii=False)},"
    content = content.rstrip(",")
    new_text = text[:open_idx + 1] + content + tail + text[close_idx:]

    updated = json.loads(new_text)  # raises if the splice broke the syntax
    for k, v in original.items():
        if k == key:
            continue
        if updated.get(k) != v:
            raise CampaignError(f"append would have altered {k!r}; aborted")
    for k, v in existing.items():
        if updated[key].get(k) != v:
            raise CampaignError(f"append would have altered {key}.{k!r}; aborted")

    with open(path, "w", encoding="utf-8") as f:
        f.write(new_text)
    return list(todo)


def list_campaigns():
    """[(id, Campaign)] for every campaign folder holding a canon_aliases.json.

    Multi-vault ids are "<world>/<campaign>"; single-vault ids are the folder name.
    """
    out = []
    try:
        if multi_vault():
            pattern = os.path.join(vaults_root_source()[0], "*",
                                   campaigns_subdir(), "*", ALIASES_FILENAME)
        else:
            pattern = os.path.join(campaigns_dir(), "*", ALIASES_FILENAME)
    except CampaignError:
        return []  # unconfigured vault: the GUI shows setup_hint() instead of dying
    for path in sorted(glob.glob(pattern)):
        camp_dir = os.path.dirname(path)
        slug = os.path.basename(camp_dir)
        if multi_vault():
            world = os.path.basename(os.path.dirname(os.path.dirname(camp_dir)))
            slug = f"{world}/{slug}"
        try:
            out.append((slug, Campaign.load(slug)))
        except CampaignError:
            continue  # unreadable file: skip rather than take the whole GUI down
    return out


def load(slug):
    return Campaign.load(slug)


def add_campaign_argument(parser, required=True):
    parser.add_argument("--campaign", required=required,
                        help="campaign id: <world>/<campaign> with GM_VAULTS_ROOT, "
                             "else the folder name under the vault's campaigns dir")


# ── CLI ───────────────────────────────────────────────────────────────────────
# transcribe.sh calls `paths` so the path rules live in exactly one place.

def _cmd_list():
    for slug, c in list_campaigns():
        print(f"{slug}\t{c.label}\t{c.system}")


def _cmd_paths(slug, session):
    c = Campaign.load(slug)
    out = {
        "CAMPAIGN":        c.slug,
        "CAMPAIGN_LABEL":  c.label,
        "CAMPAIGN_SYSTEM": c.system,
        "CAMPAIGN_WORLD":  c.world,
        "CAMPAIGN_ROOT":   c.root,
        "ALIASES_FILE":    c.aliases_path,
        "TRANSCRIPTS_DIR": c.transcripts_dir,
        "CHUNKS_DIR":      c.chunks_dir,
        "AUDIO_BASE":      c.audio_dir_base,
        "DB_PATH":         c.db_path,
    }
    if session is not None:
        out.update({
            "SESSION":         str(int(session)),
            "SESSION_DIR":     c.session_dir(session),
            "TRANSCRIPT_MD":   c.transcript_path(session),
            "CHUNKS_FILE":     c.chunks_path(session),
            "AUDIO_DIR":       c.audio_dir(session),
            "WORK_DIR":        c.work_dir(session),
        })
    for k, v in out.items():
        print(f"{k}={_shquote(v)}")


def _shquote(s):
    return "'" + str(s).replace("'", "'\\''") + "'"


def _cmd_doctor():
    """Print what every root resolved to and where it came from."""
    cfg = config_path()
    print(f"config file    {cfg}" + ("" if os.path.isfile(cfg) else "   (not present)"))
    print()

    def row(label, path, src):
        mark = "ok     " if os.path.isdir(path) else "missing"
        print(f"{label:<14} {path}")
        print(f"{'':<14} {mark}  from {src}")

    if multi_vault():
        root, src = vaults_root_source()
        row("vaults root", root, src)
        worlds = sorted(d for d in glob.glob(os.path.join(root, "*"))
                        if os.path.isdir(os.path.join(d, campaigns_subdir())))
        print(f"{'worlds':<14} {', '.join(os.path.basename(w) for w in worlds) or '(none)'}")
        if config().get("vault") or os.environ.get("GM_VAULT") or os.environ.get("VAULT"):
            print(f"{'':<14} note: a single vault is also set; the vaults root wins")
    else:
        try:
            vault, src = vault_root_source()
        except CampaignError as exc:
            print("vault          NOT CONFIGURED")
            print()
            print(exc)
            return 2
        row("vault", vault, src)
        row("campaigns", campaigns_dir(), f"vault + {campaigns_subdir()!r}")
    row("audio root", *audio_root_source())
    row("chroma root", *chroma_root_source())
    row("work root", *work_root_source())
    try:
        dev, dev_src = whisper_device_source()
    except CampaignError as exc:
        print(f"{'whisper dev':<14} INVALID\n{'':<14} {exc}")
    else:
        if dev is None:
            print(f"{'whisper dev':<14} not set (whisper-cli uses its own default, device 0)")
        else:
            print(f"{'whisper dev':<14} {dev}")
            print(f"{'':<14} set    from {dev_src}")
    print()

    camps = list_campaigns()
    if not camps:
        print(setup_hint())
        return 1
    print(f"{len(camps)} campaign(s):")
    for slug, c in camps:
        flag = "" if c.has_review_only else "   (no review_only yet)"
        print(f"  {slug:<24} {c.label}  [{c.system or 'system unset'}]{flag}")
    return 0


def main(argv=None):
    import argparse
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[1])
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("list", help="slug<TAB>label<TAB>system per campaign")
    sub.add_parser("doctor", help="show what every root resolved to, and from where")
    sub.add_parser("whisper-device",
                   help="print the configured whisper-cli device index, or nothing")
    pp = sub.add_parser("paths", help="shell-eval-able KEY=value path resolution")
    add_campaign_argument(pp)
    pp.add_argument("--session", type=int)
    a = ap.parse_args(argv)
    try:
        if a.cmd == "list":
            _cmd_list()
        elif a.cmd == "doctor":
            return _cmd_doctor()
        elif a.cmd == "whisper-device":
            dev, _ = whisper_device_source()
            if dev is not None:
                print(dev)
        else:
            _cmd_paths(a.campaign, a.session)
    except CampaignError as exc:
        sys.stderr.write(f"ERROR: {exc}\n")
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
