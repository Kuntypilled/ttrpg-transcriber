"""Print os.path.abspath(os.path.expanduser(x)) for each string in a JSON list.

Used only by the parity tests. Usage: python3 -I -B expand_paths.py '<json list>'
"""
import json
import os
import sys

print(json.dumps([os.path.abspath(os.path.expanduser(x)) for x in json.loads(sys.argv[1])],
                 ensure_ascii=False))
