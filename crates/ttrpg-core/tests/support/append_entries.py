"""Run tests/oracle/campaign.py's append_object_entries on a file, in place.

Used only by the parity tests. Usage:
    python3 -I -B append_entries.py <oracle dir> <file> <key> '<entries as a JSON object>'
Prints {"added": [...]} or {"error": "..."} as JSON.
"""
import json
import sys

legacy_dir, path, key, entries = sys.argv[1], sys.argv[2], sys.argv[3], json.loads(sys.argv[4])
sys.path.insert(0, legacy_dir)
import campaign as m  # noqa: E402

try:
    print(json.dumps({"added": m.append_object_entries(path, key, entries)}))
except Exception as exc:  # the original raises CampaignError or a JSON error
    print(json.dumps({"error": str(exc)}))
