"""Dump what tests/oracle/campaign.py computes for one campaign, as JSON.

Used only by the parity tests. Usage:
    python3 -I -B dump_campaign.py <oracle dir> <campaign id> [session ...]
"""
import json
import sys

oracle_dir, slug, sessions = sys.argv[1], sys.argv[2], [int(s) for s in sys.argv[3:]]
sys.path.insert(0, oracle_dir)
import campaign as m  # noqa: E402

c = m.load(slug)
out = {
    "slug": c.slug,
    "root": c.root,
    "label": c.label,
    "system": c.system,
    "world": c.world,
    "aliases_path": c.aliases_path,
    "transcripts_dir": c.transcripts_dir,
    "chunks_dir": c.chunks_dir,
    "audio_dir_base": c.audio_dir_base,
    "db_path": c.db_path,
    "seed_prompt": c.seed_prompt,
    "speaker_map": [list(r) for r in c.speaker_map],
    "players": [list(kv) for kv in c.players.items()],
    "pcs": c.pcs,
    "discord_names": [[k, v.get("player"), v.get("character"), v.get("role")]
                      for k, v in c.discord_names.items() if isinstance(v, dict)],
    "names": c.names,
    "aliases": [[k, v] for k, v in c.aliases.items()],
    "review_only": [[k, v] for k, v in c.review_only.items()],
    "has_review_only": c.has_review_only,
    "sessions": {
        str(n): {
            "session_dir": c.session_dir(n),
            "transcript": c.transcript_path(n),
            "chunks": c.chunks_path(n),
            "audio": c.audio_dir(n),
            "work": c.work_dir(n),
        }
        for n in sessions
    },
}
print(json.dumps(out, ensure_ascii=False))
