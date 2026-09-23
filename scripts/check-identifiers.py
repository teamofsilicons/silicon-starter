#!/usr/bin/env python3
"""Stdlib-only checks for the offline discussion-author migration."""
import argparse
import copy
from decimal import Decimal
import json
import os
from pathlib import Path
import runpy
import subprocess
import tempfile
from urllib.parse import quote

script = Path(__file__).with_name("migrate-identifiers.py").resolve()
module = runpy.run_path(str(script))
migrate = module["migrate"]
connection_env = module["connection_env"]
connection = connection_env({"STARTER_DATABASE_URL": "postgresql://user:p%40ss%3Aword@[::1]:5433/db%20name?sslmode=require&connect_timeout=9", "PGHOSTADDR": "wrong", "PGSERVICE": "wrong"})
assert connection["PGHOST"] == "::1" and connection["PGPORT"] == "5433"
assert connection["PGUSER"] == "user" and connection["PGPASSWORD"] == "p@ss:word"
assert connection["PGDATABASE"] == "db name" and connection["PGSSLMODE"] == "require"
assert connection["PGCONNECT_TIMEOUT"] == "9" and "PGHOSTADDR" not in connection and "PGSERVICE" not in connection
assert connection_env({"PGDATABASE": "postgres", "PGHOST": "/tmp"}) == {"PGDATABASE": "postgres", "PGHOST": "/tmp"}
assert connection_env({"DATABASE_URL": "postgresql://host/db?password=a+b%26c"})["PGPASSWORD"] == "a+b&c"
assert connection_env({"DATABASE_URL": "postgresql://host:0/db"})["PGPORT"] == "0"
for invalid in ("mysql://user:secret@host/db", "postgresql://user:secret@host/db?unsupported=yes", "postgresql://user:secret@host/db?sslmode=require&sslmode=disable", "postgresql://user:secret@host:bad/db", "postgresql:///db", "postgresql://user:%FF@host/db", "postgresql://user:secret%ZZ@host/db"):
    try:
        connection_env({"DATABASE_URL": invalid})
    except ValueError as error:
        assert "secret" not in str(error) and invalid not in str(error)
    else:
        raise AssertionError("invalid PostgreSQL URL accepted")
mapping = {
    "scope_key": "production",
    "actors": [
        {"kind": "carbon", "old_id": "alice0", "new_id": "c:alice0"},
        {"kind": "silicon", "old_id": "research:tos", "new_id": "si:research", "org_id": "tos"},
        {"kind": "carbon", "old_id": "c:existing", "new_id": "c:existing"},
    ],
}
snapshot = {
    "starters": {"tos.example": {"owner": "tos", "yaml": "id: research:tos\napps: [tos>dm]"}},
    "versions": {"tos.example": [{"notes": "alice0 and research:tos", "commit": "frozen"}]},
    "discussions": {"tos.example": [
        {"id": "uuid-1", "author": "alice0", "body": "research:tos's quoted \\ body"},
        {"id": "uuid-2", "author": "research:tos", "body": "alice0", "parent_id": "uuid-1"},
        {"id": "uuid-3", "author": "authenticated-user", "body": "unchanged"},
        {"id": "uuid-4", "author": "c:existing", "body": "already canonical"},
    ]},
    "bundles": {"tos.example": "frozen-base64-bytes"},
    "briefcase_entries": {"tos.example": "unchanged-resource-uuid"},
}
before = copy.deepcopy(snapshot)
expected = copy.deepcopy(snapshot)
expected["discussions"]["tos.example"][0]["author"] = "c:alice0"
expected["discussions"]["tos.example"][1]["author"] = "si:research"
updated, count = migrate(snapshot, mapping, "production")
assert updated == expected and count == 2 and snapshot == before
assert migrate(updated, mapping, "production") == (expected, 0)
assert migrate({}, {"scope_key": "production", "actors": []}, "production") == ({}, 0)


def rejected(candidate=mapping, data=snapshot, scope="production"):
    try:
        migrate(data, candidate, scope)
    except ValueError:
        return
    raise AssertionError("unsafe migration accepted")


rejected(scope="testing:other-world")
rejected({**mapping, "scope_key": "testing:other-world"})
rejected({**mapping, "actors": mapping["actors"][1:]})
rejected({**mapping, "actors": mapping["actors"] + [mapping["actors"][0]]})
for actor in (
    {"kind": "carbon", "old_id": "other", "new_id": "c:alice0"},
    {"kind": "carbon", "old_id": "c:alice0", "new_id": "c:alice0"},
    {"kind": "silicon", "old_id": "other:other-org", "new_id": "si:research", "org_id": "other-org"},
    {"kind": "silicon", "old_id": "research:tos", "new_id": "si:research", "org_id": "wrong-org"},
    {"kind": "silicon", "old_id": "research:tos", "new_id": "si:research"},
    {"kind": "carbon", "old_id": "research:tos", "new_id": "c:research"},
    {"kind": "silicon", "old_id": "c:alice0", "new_id": "si:alice0", "org_id": "tos"},
    {"kind": "carbon", "old_id": "alice0", "new_id": "si:alice0"},
    {"kind": "carbon", "old_id": "authenticated-user", "new_id": "c:authenticated-user"},
):
    rejected({**mapping, "actors": mapping["actors"] + [actor]})
collision = copy.deepcopy(snapshot)
collision["discussions"]["tos.example"].append({"author": "c:alice0"})
mixed, count = migrate(collision, mapping, "production")
assert count == 2 and mixed["discussions"]["tos.example"][-1] == {"author": "c:alice0"}
assert collision["discussions"]["tos.example"][0]["author"] == "alice0"
for discussions in ([], {"tos.example": {}}, {"tos.example": [{}]}, {"tos.example": [{"author": 3}]}):
    rejected(data={"discussions": discussions})
print("Identifier checks passed: exact preservation, rerun, world fence, missing maps, kinds, ownership, and collisions.")

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--postgres-bin", type=Path, help="also exercise a disposable local PostgreSQL server")
args = parser.parse_args()
if args.postgres_bin:
    with tempfile.TemporaryDirectory(prefix="starter-identifiers-", dir="/tmp") as directory:
        root = Path(directory)
        env = {key: value for key, value in os.environ.items()
               if not key.startswith("PG") and key not in ("STARTER_DATABASE_URL", "DATABASE_URL")}
        env.update(PGDATABASE="postgres", PGHOST=str(root), PGPORT="5432")

        def run(*command, ok=True):
            result = subprocess.run(command, env=env, text=True, capture_output=True)
            assert (result.returncode == 0) == ok, (command, result.stdout, result.stderr)
            return result.stdout.strip()

        def sql(query):
            return run("psql", "-X", "-qAt", "-v", "ON_ERROR_STOP=1", "-c", query)

        run(str(args.postgres_bin / "initdb"), "-D", str(root / "pg"), "-A", "trust", "--no-locale")
        pg_ctl = (str(args.postgres_bin / "pg_ctl"), "-D", str(root / "pg"))
        run(*pg_ctl, "-l", str(root / "server.log"), "-o", f"-h '' -k {root}", "-w", "start")
        try:
            source = copy.deepcopy(snapshot)
            source["precise"] = "12345678901234567890.12345678901234567890"
            source["discussions"]["tos.example"][0]["precise"] = source["precise"]
            source["discussions"]["empty"] = []
            original = json.dumps(source).replace('"' + source["precise"] + '"', source["precise"])
            literal = module["sql_json_text"]
            sql("CREATE TABLE starter_state (id smallint PRIMARY KEY, payload jsonb NOT NULL, updated_at timestamptz DEFAULT now());")
            sql(f"INSERT INTO starter_state (id, payload) VALUES (1, {literal(original)});")
            mapfile = root / "map.json"
            mapfile.write_text(json.dumps(mapping))
            command = ("python3", str(script), "--scope-key", "production", "--map", str(mapfile))
            env["STARTER_DATABASE_URL"] = f"postgresql:///postgres?host={quote(str(root), safe='')}&port=5432"
            assert "Preview: 2" in run(*command)
            before = sql("SELECT payload::text FROM starter_state WHERE id=1;")
            assert json.loads(before, parse_float=Decimal) == json.loads(original, parse_float=Decimal)
            backup = root / "snapshot.json"
            assert "Applied: 2" in run(*command, "--apply", "--backup", str(backup))
            assert backup.read_text() == before + "\n" and backup.stat().st_mode & 0o777 == 0o600
            current = sql("SELECT payload::text FROM starter_state WHERE id=1;")
            expected = json.loads(original, parse_float=Decimal)
            expected["discussions"]["tos.example"][0]["author"] = "c:alice0"
            expected["discussions"]["tos.example"][1]["author"] = "si:research"
            assert json.loads(current, parse_float=Decimal) == expected
            assert "Preview: 0" in run(*command)
            assert "Applied: 0" in run(*command, "--apply", "--backup", str(root / "rerun.json"))
            run(*command, "--apply", "--backup", str(backup), ok=False)
            run(*command, "--scope-key", "testing:other-world", "--apply", "--backup", str(root / "wrong.json"), ok=False)
            mapfile.write_text(json.dumps({**mapping, "actors": mapping["actors"][1:]}))
            run(*command, "--apply", "--backup", str(root / "missing.json"), ok=False)
            assert not (root / "wrong.json").exists() and not (root / "missing.json").exists()
            assert sql("SELECT payload::text FROM starter_state WHERE id=1;") == current
            actors = {actor["old_id"]: actor["new_id"] for actor in mapping["actors"]}
            # The same update implementation refuses the now-stale snapshot.
            assert sql(module["update_sql"](before, actors)) == ""
            assert sql("SELECT payload::text FROM starter_state WHERE id=1;") == current
            print("PostgreSQL checks passed: preview, apply, private/exclusive backup, SQL escaping, numeric precision, rerun, and no writes on stale/missing/wrong-world input.")
        finally:
            run(*pg_ctl, "-m", "immediate", "-w", "stop")
