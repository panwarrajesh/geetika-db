"""Minimal script using the Python SDK.

    PowerShell:  $env:RAJESHDB_URL="http://127.0.0.1:8081"; $env:RAJESHDB_PROJECT="<32-char id>"; $env:RAJESHDB_API_KEY="rdb_..."
    python examples/python/example.py
"""
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "sdk" / "python"))
from rajeshdb import Client, RajeshDBError  # noqa: E402

client = Client(os.environ["RAJESHDB_URL"], os.environ["RAJESHDB_API_KEY"], os.environ["RAJESHDB_PROJECT"], env=os.environ.get("RAJESHDB_ENV", "dev"))
people = client.collection("people")
try:
    new_id = people.insert({"name": "Asha", "city": "Delhi", "age": 30})
    print("inserted", new_id)
    print("found:", people.find(filter={"city": "Delhi"}, limit=10)["documents"])
    print("updated:", people.update({"city": "Delhi"}, {"age": 31}))
    print("deleted:", people.delete(new_id))
except RajeshDBError as e:
    print(f"RajeshDB error {e.status} {e.code}: {e.message}")
