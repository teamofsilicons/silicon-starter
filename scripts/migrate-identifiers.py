#!/usr/bin/env python3
"""Preview/apply discussion-author IDs from an approved, world-scoped IAM map."""
import argparse
import copy
import json
import os
from pathlib import Path
import re
import subprocess
from urllib.parse import parse_qsl, unquote, urlsplit


def migrate(snapshot, mapping, scope_key):
    if not isinstance(mapping, dict) or set(mapping) != {"scope_key", "actors"}:
        raise ValueError("mapping must contain scope_key and actors")
    if not scope_key or mapping["scope_key"] != scope_key:
        raise ValueError("mapping scope_key does not match the selected world")
    if not isinstance(mapping["actors"], list):
        raise ValueError("mapping actors must be a list")
    actors = {}
    targets = set()
    for actor in mapping["actors"]:
        if not isinstance(actor, dict) or set(actor) - {"kind", "old_id", "new_id", "org_id"}:
            raise ValueError("invalid actor mapping fields")
        kind, old, new = (actor.get(key) for key in ("kind", "old_id", "new_id"))
        if kind not in ("carbon", "silicon") or not isinstance(old, str) or not isinstance(new, str):
            raise ValueError("each actor requires kind, old_id, and new_id")
        prefix, limit = ("c:", 30) if kind == "carbon" else ("si:", 50)
        handle = rf"[a-z0-9_-]{{3,{limit}}}"
        if not re.fullmatch(prefix + handle, new) or old == "authenticated-user":
            raise ValueError("invalid canonical actor ID or reserved legacy author")
        if kind == "silicon" and (not isinstance(actor.get("org_id"), str) or not actor["org_id"]):
            raise ValueError("Silicon mappings require verified org_id")
        if old.startswith(("c:", "si:")):
            if old != new:
                raise ValueError("canonical actor mappings must retain the same kind and ID")
        elif kind == "carbon":
            if not re.fullmatch(handle, old):
                raise ValueError("invalid legacy Carbon ID")
        else:
            old_handle, separator, old_org = old.partition(":")
            if not separator or not re.fullmatch(handle, old_handle) or old_org != actor["org_id"]:
                raise ValueError("legacy Silicon ID disagrees with verified org_id")
        if old in actors or new in targets:
            raise ValueError("duplicate or colliding actor mapping")
        actors[old] = new
        targets.add(new)

    if not isinstance(snapshot, dict) or not isinstance(snapshot.get("discussions", {}), dict):
        raise ValueError("invalid Starter snapshot discussions")
    result = copy.deepcopy(snapshot)
    changed = 0
    for discussions in result.get("discussions", {}).values():
        if not isinstance(discussions, list):
            raise ValueError("invalid discussion list")
        for discussion in discussions:
            if not isinstance(discussion, dict) or not isinstance(discussion.get("author"), str):
                raise ValueError("invalid discussion author")
            old = discussion["author"]
            if old == "authenticated-user":
                continue
            new = actors.get(old, old if old in targets else None)
            if new is None:
                raise ValueError("unmapped discussion author; reconcile with IAM inventory")
            discussion["author"] = new
            changed += new != old
    return result, changed


def connection_env(source):
    env = source.copy()
    database = env.get("STARTER_DATABASE_URL") or env.get("DATABASE_URL") or env.get("PGDATABASE")
    if not database:
        raise ValueError("set STARTER_DATABASE_URL, DATABASE_URL, or PGDATABASE explicitly")
    if "://" not in database:
        if env.get("STARTER_DATABASE_URL") or env.get("DATABASE_URL") or "=" in database:
            raise ValueError("database URL must use postgresql:// or postgres://")
        return env
    options = {
        "host": "PGHOST", "hostaddr": "PGHOSTADDR", "port": "PGPORT", "dbname": "PGDATABASE",
        "user": "PGUSER", "password": "PGPASSWORD", "sslmode": "PGSSLMODE",
        "sslcert": "PGSSLCERT", "sslkey": "PGSSLKEY", "sslrootcert": "PGSSLROOTCERT",
        "sslcrl": "PGSSLCRL", "sslcrldir": "PGSSLCRLDIR", "channel_binding": "PGCHANNELBINDING",
        "connect_timeout": "PGCONNECT_TIMEOUT", "client_encoding": "PGCLIENTENCODING",
        "options": "PGOPTIONS", "application_name": "PGAPPNAME",
        "target_session_attrs": "PGTARGETSESSIONATTRS",
    }
    try:
        if re.search(r"%(?![0-9a-fA-F]{2})|[\x00-\x1f\x7f]", database):
            raise ValueError()
        url = urlsplit(database)
        query = parse_qsl(url.query.replace("+", "%2B"), keep_blank_values=True, strict_parsing=True, errors="strict")
        if url.scheme not in ("postgres", "postgresql") or url.fragment:
            raise ValueError()
        if len(dict(query)) != len(query) or any(key not in options for key, _ in query):
            raise ValueError()
        env.pop("PGSERVICE", None)
        env.pop("PGHOSTADDR", None)
        env.update(PGHOST=unquote(url.hostname or "", errors="strict"), PGPORT=str(url.port if url.port is not None else 5432),
                   PGDATABASE=unquote(url.path.removeprefix("/"), errors="strict"))
        if url.username is not None:
            env["PGUSER"] = unquote(url.username, errors="strict")
        if url.password is not None:
            env["PGPASSWORD"] = unquote(url.password, errors="strict")
        env.update((options[key], value) for key, value in query)
        if not (env.get("PGHOST") or env.get("PGHOSTADDR")) or not env.get("PGDATABASE"):
            raise ValueError()
        if any("\0" in value for value in env.values()):
            raise ValueError()
    except ValueError:
        raise ValueError("invalid PostgreSQL URL, missing host/database, or unsupported query option") from None
    return env


def psql(sql):
    env = connection_env(os.environ)
    result = subprocess.run(
        ["psql", "-X", "-v", "ON_ERROR_STOP=1", "-qAt", "--no-password"],
        input="SET standard_conforming_strings = on;\n" + sql,
        env=env, text=True, capture_output=True,
    )
    if result.returncode:
        # SQL errors can contain snapshot contents; do not expose captured diagnostics.
        raise ValueError("psql failed; check database connectivity, permissions, and schema")
    return result.stdout.strip()


def sql_json_text(value):
    return "'" + value.replace("'", "''") + "'::jsonb"


def update_sql(original, actors):
    # Keep all existing JSONB values in PostgreSQL, including precise numeric fields.
    return f"""
WITH actor_map AS (SELECT {sql_json_text(json.dumps(actors))} AS ids)
UPDATE starter_state SET payload = jsonb_set(payload, '{{discussions}}', (
    SELECT jsonb_object_agg(starter_id, (
        SELECT coalesce(jsonb_agg(
            CASE WHEN actor_map.ids ? (discussion->>'author')
                THEN jsonb_set(discussion, '{{author}}', actor_map.ids->(discussion->>'author'))
                ELSE discussion END ORDER BY position
        ), '[]'::jsonb)
        FROM jsonb_array_elements(entries) WITH ORDINALITY AS d(discussion, position)
    ))
    FROM jsonb_each(payload->'discussions') AS s(starter_id, entries)
)), updated_at = now()
FROM actor_map
WHERE id = 1 AND payload = {sql_json_text(original)}
RETURNING id;
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--map", required=True, type=Path, dest="mapping")
    parser.add_argument("--scope-key", required=True, help="IAM world for this database, e.g. production")
    parser.add_argument("--apply", action="store_true")
    parser.add_argument("--backup", type=Path, help="new private snapshot file, required with --apply")
    args = parser.parse_args()
    if args.apply and args.backup is None:
        parser.error("--apply requires --backup with a new file path")
    mapping = json.loads(args.mapping.read_text())
    original = psql("SELECT payload::text FROM starter_state WHERE id = 1;")
    if not original:
        raise ValueError("starter_state row 1 is missing")
    snapshot = json.loads(original)
    _, changed = migrate(snapshot, mapping, args.scope_key)
    if args.apply:
        descriptor = os.open(args.backup, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "w") as backup:
            backup.write(original + "\n")
            backup.flush()
            os.fsync(backup.fileno())
        if changed:
            actors = {actor["old_id"]: actor["new_id"] for actor in mapping["actors"]}
            result = psql(update_sql(original, actors))
            if result != "1":
                raise ValueError("snapshot changed concurrently; no row updated, backup retained")
    print(f"{'Applied' if args.apply else 'Preview'}: {changed} discussion author(s) changed.")


if __name__ == "__main__":
    try:
        main()
    except json.JSONDecodeError:
        raise SystemExit("Invalid JSON in mapping or database snapshot") from None
    except ValueError as error:
        raise SystemExit(str(error)) from None
    except OSError:
        raise SystemExit("Could not read mapping, run psql, or create a new private backup") from None
