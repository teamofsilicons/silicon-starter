#!/usr/bin/env python3
"""Smoke-check a built CLI without production writes: python3 scripts/check-cli.py /path/to/starter."""
import base64
import hashlib
import http.server
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import zipfile
from urllib.parse import parse_qs, urlsplit

binary = pathlib.Path(sys.argv[1]).resolve()
with tempfile.TemporaryDirectory(prefix="starter-cli-check-") as directory:
    root = pathlib.Path(directory)
    env = {**os.environ, "SILICON_HOME": str(root), "HOME": str(root), "STARTER_NO_DAEMON": "1"}
    env.pop("STARTER_API_URL", None)
    for key in ("STARTER_PROFILE", "STARTER_WORLD", "SILICON_ORG"):
        env.pop(key, None)
    for key in list(env):
        if key.startswith("GIT_"):
            env.pop(key)

    def run(*args, cwd=root):
        return subprocess.run(args, cwd=cwd, env=env, text=True, capture_output=True, check=True).stdout

    metadata = json.loads(run(str(binary), "iam", "--json"))
    assert metadata["app_id"] == "starter", metadata
    assert run(str(binary), "iam").splitlines()[0] == "starter"
    assert metadata["base_url"] == "https://backend.starter.teamofsilicons.com", metadata
    # Check native home resolution (including Windows without HOME) separately
    # from SILICON_HOME, then restore the isolated app home for the API checks.
    native_home = root / "native-home"
    native_home.mkdir()
    env.pop("SILICON_HOME")
    env["USERPROFILE" if os.name == "nt" else "HOME"] = str(native_home)
    if os.name == "nt":
        env.pop("HOME", None)
    run(str(binary), "webhook", "https://example.invalid/starter")
    assert (native_home / ".starter/webhook.json").is_file()
    run(str(binary), "unhook")
    env["SILICON_HOME"] = str(root)
    run(str(binary), "webhook", "https://example.invalid/starter")
    assert (root / ".starter/webhook.json").is_file()
    run(str(binary), "unhook")
    source = root / "source"
    source.mkdir()
    run("git", "init", "-b", "main", cwd=source)
    (source / "README.md").write_text("starter content\n")
    (source / "dividers.txt").write_text("Heading\n=======\n\n====================\nSection\n====================\n")
    (source / "fixture.bin").write_bytes(b"\0\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> main\n")
    run("git", "add", "README.md", "dividers.txt", "fixture.bin", cwd=source)
    run("git", "-c", "user.name=Test", "-c", "user.email=test@example.invalid", "commit", "-m", "Initial", cwd=source)
    commit = run("git", "rev-parse", "HEAD", cwd=source).strip()
    first_commit = commit
    bundle = root / "starter.bundle"
    run("git", "bundle", "create", str(bundle), "refs/heads/main", cwd=source)
    requests = []
    blocks = {}

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            requests.append((self.path, self.headers.get("x-starter-session")))
            unauthorized = self.headers.get("x-starter-session") == "opaque:legacy>session"
            if self.path.startswith("/api/v1/blocks/"):
                block_id = self.path.removeprefix("/api/v1/blocks/").split("/")[0]
                block = blocks[block_id]
                self.reply([{"version": block["version"]}] if self.path.endswith("/versions") else block)
                return
            reference = parse_qs(urlsplit(self.path).query).get("ref", [None])[0]
            selected_commit = first_commit if reference == "1.0" else reference or commit
            payload = {"error": "session expired"} if unauthorized else {
                "/api/v1/starters": [],
                "/api/v1/starters/tos.example": {"id": "tos.example", "owner": "tos"},
                "/api/v1/starters/tos.example/archive": {
                    "bundle_base64": base64.b64encode(bundle.read_bytes()).decode(), "commit": selected_commit,
                },
            }[self.path.split("?")[0]]
            self.reply(payload, 401 if unauthorized else 200)

        def do_POST(self):
            assert self.path == "/api/v1/blocks" and self.headers.get("x-starter-session") == "test-session"
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            content = body["text"].encode() if "text" in body else base64.b64decode(body["archive_base64"])
            body.update(kind=body["id"].split(":")[0], version=hashlib.sha256(content).hexdigest())
            blocks[body["id"]] = body
            self.reply(body)

        def reply(self, payload, status=200):
            body = json.dumps(payload).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        api = f"http://127.0.0.1:{server.server_port}"
        assert json.loads(run(str(binary), "--api", api, "iam", "--json"))["base_url"] == api
        missing = subprocess.run([str(binary), "--api", api, "pull"], cwd=root, env=env, text=True, capture_output=True)
        assert missing.returncode != 0 and "pull requires a starter id on first use" in missing.stderr, missing
        assert not requests, requests
        missing_auth = subprocess.run([str(binary), "--api", api, "pull", "tos.example", "--defaults"], cwd=root, env=env, text=True, capture_output=True)
        assert missing_auth.returncode != 0 and "not authenticated" in missing_auth.stderr, missing_auth
        assert not requests, requests
        run(str(binary), "--api", api, "list")
        run(str(binary), "--api", api, "download", "tos.example", "--dir", "anonymous")
        assert (root / "anonymous/README.md").read_text() == "starter content\n"
        shutil.rmtree(root / "anonymous")
        (root / ".starter").mkdir(exist_ok=True)
        session_file = root / ".starter/profiles/default" / hashlib.sha256(f"{api}\nproduction".encode()).hexdigest() / "session.json"
        session_file.parent.mkdir(parents=True, exist_ok=True)
        def write_session(token):
            session_file.write_text(json.dumps({"api":api,"profile":"default","world":"production","world_fingerprint":"production:1","session_id":token,"context_id":"828c7fc8-04cb-409a-829a-6f52756b3b24","actor":{"type":"silicon","public_id":"si:tester"},"org_id":"tos"}))
            session_file.chmod(0o600)
        write_session("test-session")
        run(str(binary), "--api", api, "list")
        run(str(binary), "--api", api, "show", "tos.example")
        run(str(binary), "--api", api, "download", "tos.example")
        assert requests == [
            ("/api/v1/starters", None),
            ("/api/v1/starters/tos.example/archive", None),
            ("/api/v1/starters", "test-session"),
            ("/api/v1/starters/tos.example", "test-session"),
            ("/api/v1/starters/tos.example/archive", "test-session"),
        ], requests
        for invalid_commit in ("9.9", "abcde"):
            invalid = subprocess.run([str(binary), "--api", api, "download", f"tos.example@{invalid_commit}", "--dir", "invalid-version"], cwd=root, env=env, text=True, capture_output=True)
            assert invalid.returncode != 0 and "full Git commit ID" in invalid.stderr, invalid
            assert not (root / "invalid-version").exists()
        write_session("opaque:legacy>session")
        expired = subprocess.run([str(binary), "--api", api, "list"], cwd=root, env=env, text=True, capture_output=True)
        assert expired.returncode != 0 and "starter login <SLT>" in expired.stderr, expired
        assert requests[-1] == ("/api/v1/starters", "opaque:legacy>session"), requests
        assert json.loads(session_file.read_text())["session_id"] == "opaque:legacy>session"
        write_session("test-session")
        assert (root / "example/README.md").read_text() == "starter content\n"
        assert json.loads((root / "example/.git/starter.json").read_text())["id"] == "tos.example"
        checkout = root / "example"
        nested = checkout / "nested"
        nested.mkdir()
        for cwd in (checkout, nested):
            content = f"updated from {cwd.name}\n"
            (source / "README.md").write_text(content)
            run("git", "-c", "user.name=Test", "-c", "user.email=test@example.invalid", "commit", "-am", "Update", cwd=source)
            commit = run("git", "rev-parse", "HEAD", cwd=source).strip()
            run("git", "bundle", "create", str(bundle), "refs/heads/main", cwd=source)
            run(str(binary), "--api", api, "pull", cwd=cwd)
            assert (checkout / "README.md").read_text() == content
            assert run("git", "rev-parse", "HEAD", cwd=checkout).strip() == commit
            assert requests[-1] == ("/api/v1/starters/tos.example/archive?mode=pull", "test-session"), requests
        assert not list(nested.iterdir()), list(nested.iterdir())
        binding = json.loads((checkout / ".git/starter.json").read_text())
        assert binding["id"] == "tos.example" and binding["mode"] == "download" and "auto_update" not in binding, binding
        registry = root / ".starter/registry.json"
        assert [entry["path"] for entry in json.loads(registry.read_text())] == [str(checkout.resolve())]
        run(str(binary), "--api", api, "pull", "tos.example", "--dir", "development", "--defaults")
        run(str(binary), "--api", api, "download", "tos.example@1.0", "--dir", "pinned")
        run(str(binary), "--api", api, "download", "tos.example", "--dir", "disabled")
        run(str(binary), "--api", api, "update", "off", cwd=root / "disabled")
        assert [entry["path"] for entry in json.loads(registry.read_text())] == [str(checkout.resolve())]
        entries = json.loads(registry.read_text())
        for name in ("development", "pinned", "disabled"):
            legacy = json.loads((root / name / ".git/starter.json").read_text())
            legacy["auto_update"] = name != "disabled"
            entries.append({"path": str(root / name), "binding": legacy})
        entries.append({"path": str(root / "deleted"), "binding": binding})
        registry.write_text(json.dumps(entries))
        run(str(binary), "--api", api, "daemon", "--once")
        assert [entry["path"] for entry in json.loads(registry.read_text())] == [str(checkout.resolve())]
        assert "auto_update" not in registry.read_text()
        # Removing membership alone disables updates, and manual refresh does not re-enable it.
        registry.write_text("[]")
        before_requests = len(requests)
        run(str(binary), "--api", api, "daemon", "--once")
        assert len(requests) == before_requests
        run(str(binary), "--api", api, "update", "now", cwd=checkout)
        assert json.loads(registry.read_text()) == []
        run(str(binary), "--api", api, "update", "on", cwd=checkout)
        previous_commit = commit
        previous_content = (checkout / "README.md").read_text()
        # The requested revision, rather than bundle main, is merged when selecting a release.
        run(str(binary), "--api", api, "download", "tos.example", "--dir", "select-version")
        (source / "README.md").write_text("Selected release\n")
        run("git", "-c", "user.name=Test", "-c", "user.email=test@example.invalid", "commit", "-am", "Selected release", cwd=source)
        selected_commit = run("git", "rev-parse", "HEAD", cwd=source).strip()
        (source / "README.md").write_text("Newer unreleased version\n")
        run("git", "-c", "user.name=Test", "-c", "user.email=test@example.invalid", "commit", "-am", "Newer version", cwd=source)
        commit = run("git", "rev-parse", "HEAD", cwd=source).strip()
        run("git", "bundle", "create", str(bundle), "refs/heads/main", cwd=source)
        selected_checkout = root / "select-version"
        run(str(binary), "--api", api, "download", f"tos.example@{selected_commit}", cwd=selected_checkout)
        assert run("git", "rev-parse", "HEAD", cwd=selected_checkout).strip() == selected_commit
        assert (selected_checkout / "README.md").read_text() == "Selected release\n"
        downgrade = subprocess.run([str(binary), "--api", api, "download", f"tos.example@{first_commit}"], cwd=selected_checkout, env=env, text=True, capture_output=True)
        assert downgrade.returncode != 0 and "--dir" in downgrade.stderr, downgrade
        assert run("git", "rev-parse", "HEAD", cwd=selected_checkout).strip() == selected_commit
        for content in (
            "<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> main\n",
            "<<<<<<<<<< branch\nours\n",
            "||||||| parent\nbase\n",
            ">>>>>>>\n",
        ):
            (source / "README.md").write_text(content)
            run("git", "-c", "user.name=Test", "-c", "user.email=test@example.invalid", "commit", "-am", "Unresolved conflict", cwd=source)
            commit = run("git", "rev-parse", "HEAD", cwd=source).strip()
            run("git", "bundle", "create", str(bundle), "refs/heads/main", cwd=source)
            conflict = subprocess.run([str(binary), "--api", api, "pull"], cwd=checkout, env=env, text=True, capture_output=True)
            assert conflict.returncode != 0 and "update left merge conflict markers" in conflict.stderr, conflict
            assert run("git", "rev-parse", "HEAD", cwd=checkout).strip() == previous_commit
            assert (checkout / "README.md").read_text() == previous_content
        assert json.loads(registry.read_text()) == []

        # Blocks need no Git checkout, and their content hashes select immutable versions.
        gene = root / "creativity.md"
        gene.write_text("# Explore three different approaches.\n")
        run(str(binary), "--api", api, "publish", "gene:creativity", str(gene), "--org", "tos")
        assert blocks["gene:creativity"]["org_id"] == "tos"
        run(str(binary), "--api", api, "publish", "gene:direct", "--text", "Write thoughtfully.")
        for block_id, filename, yaml in (
            ("isi:worker", "isi.yaml", "isi:\n  worker: {defaults: {temperature: 0.5}}\n"),
            ("function:greet", "function.yaml", "functions:\n  greet: {params: [name], do: []}\n"),
        ):
            archive = root / "block.zip"
            with zipfile.ZipFile(archive, "w") as zipped:
                zipped.writestr(filename, yaml)
                zipped.writestr("prompts/guide.md", "Be helpful.")
                script = zipfile.ZipInfo("scripts/helper.sh")
                script.external_attr = 0o100755 << 16
                zipped.writestr(script, "#!/bin/sh\nprintf hello\n")
            run(str(binary), "--api", api, "publish", block_id, str(archive), "--org", "tos", "--visibility", "private")
            output = root / block_id.split(":")[0]
            run(str(binary), "--api", api, "download", block_id, "--dir", str(output))
            assert (output / filename).read_text() == yaml
            assert (output / "prompts/guide.md").read_text() == "Be helpful."
            if os.name != "nt":
                assert os.access(output / "scripts/helper.sh", os.X_OK)
        version = blocks["gene:creativity"]["version"]
        run(str(binary), "--api", api, "download", f"gene:creativity@{version}", "--dir", "gene-output")
        assert (root / "gene-output/creativity.md").read_bytes() == gene.read_bytes()
        assert json.loads(run(str(binary), "--api", api, "history", "gene:creativity"))[0]["version"] == version
        assert json.loads(run(str(binary), "--api", api, "publish", "history", "gene:creativity"))[0]["version"] == version
        assert json.loads(run(str(binary), "--api", api, "show", "gene:creativity"))["id"] == "gene:creativity"
        blocks["gene:creativity"]["text"] = "Tampered content"
        tampered = subprocess.run([str(binary), "--api", api, "download", "gene:creativity", "--dir", "tampered"], cwd=root, env=env, text=True, capture_output=True)
        assert tampered.returncode != 0 and "content hash" in tampered.stderr, tampered
        assert not (root / "tampered").exists()
        assert json.loads(registry.read_text()) == []

        # Enabling updates starts one detached daemon; its OS lock rejects duplicates.
        env.pop("STARTER_NO_DAEMON")
        lock = root / ".starter/daemon.lock"
        old_pid = lock.read_text().strip()
        daemon_pid = None
        try:
            run(str(binary), "--api", api, "update", "on", cwd=root / "disabled")
            for _ in range(100):
                pid = lock.read_text().strip()
                if pid and pid != old_pid:
                    daemon_pid = int(pid)
                    break
                time.sleep(0.05)
            assert daemon_pid, "download updates did not start their background daemon"
            run(str(binary), "--api", api, "update", "on", cwd=root / "disabled")
            run(str(binary), "--api", api, "daemon", "--once")
            assert int(lock.read_text()) == daemon_pid
        finally:
            if daemon_pid:
                os.kill(daemon_pid, 15)
            env["STARTER_NO_DAEMON"] = "1"
    finally:
        server.shutdown()
        server.server_close()
        thread.join()
print("CLI checks passed: IAM metadata/authenticated pull, anonymous downloads, API override, sessions, nested pull, conflict rollback, central registry migration/pruning/toggles, block publish/download/history/hash validation, single background daemon.")
