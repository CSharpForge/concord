"""Check Concord's database after a first start (used by the installer smoke test).

    python scripts/check_db.py <path to concord.db>

Exits non-zero unless the file exists, has been migrated to schema version 2 or
later, and has the tables the app needs.
"""
import sqlite3
import sys

REQUIRED = {"jobs", "job_messages", "protected"}

path = sys.argv[1]
con = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
version = con.execute("PRAGMA user_version").fetchone()[0]
tables = {r[0] for r in con.execute("SELECT name FROM sqlite_master WHERE type='table'")}
print(f"schema version {version}; tables: {sorted(tables)}")
missing = REQUIRED - tables
if version < 2 or missing:
    print(f"FAIL: expected schema version >= 2 and tables {sorted(REQUIRED)}; missing {sorted(missing)}")
    sys.exit(1)
print("database OK")
