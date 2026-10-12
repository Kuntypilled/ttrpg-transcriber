"""Stands in for a machine where chromadb is not installed: importing it fails the way Python says it."""
raise ModuleNotFoundError("No module named 'chromadb'", name="chromadb")
