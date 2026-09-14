#!/usr/bin/env python3
"""Smoke-check a built CLI without production writes: python3 scripts/check-cli.py /path/to/starter."""
import base64
import http.server
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import threading

binary = pathlib.Path(sys.argv[1]).resolve()
with tempfile.TemporaryDirectory(prefix="starter-cli-check-") as directory:
    root = pathlib.Path(directory)
    env = {**os.environ, "SILICON_HOME": str(root), "HOME": str(root)}
    env.pop("STARTER_API_URL", None)
    for key in list(env):
        if key.startswith("GIT_"):
            env.pop(key)

    def run(*args, cwd=root):
        return subprocess.run(args, cwd=cwd, env=env, text=True, capture_output=True, check=True).stdout

    metadata = json.loads(run(str(binary), "iam", "--json"))
    assert metadata["base_url"] == "https://backend.starter.teamofsilicons.com", metadata
    source = root / "source"
    source.mkdir()
    run("git", "init", "-b", "main", cwd=source)
    (source / "README.md").write_text("starter content\n")
    run("git", "add", "README.md", cwd=source)
    run("git", "-c", "user.name=Test", "-c", "user.email=test@example.invalid", "commit", "-m", "Initial", cwd=source)
    commit = run("git", "rev-parse", "HEAD", cwd=source).strip()
    bundle = root / "starter.bundle"
    run("git", "bundle", "create", str(bundle), "refs/heads/main", cwd=source)
    requests = []

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            requests.append((self.path, self.headers.get("x-starter-session")))
            payload = {
                "/api/v1/starters": [],
                "/api/v1/starters/tos.example": {"id": "tos.example", "owner": "tos"},
                "/api/v1/starters/tos.example/archive": {
                    "bundle_base64": base64.b64encode(bundle.read_bytes()).decode(), "commit": commit,
                },
            }[self.path]
            body = json.dumps(payload).encode()
            self.send_response(200)
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
        run(str(binary), "--api", api, "list")
        (root / ".starter").mkdir()
        (root / ".starter/session").write_text("test-session\n")
        run(str(binary), "--api", api, "list")
        run(str(binary), "--api", api, "show", "tos.example")
        run(str(binary), "--api", api, "download", "tos.example")
        assert requests == [
            ("/api/v1/starters", None),
            ("/api/v1/starters", "test-session"),
            ("/api/v1/starters/tos.example", "test-session"),
            ("/api/v1/starters/tos.example/archive", "test-session"),
        ], requests
        assert (root / "example/README.md").read_text() == "starter content\n"
        assert json.loads((root / "example/.git/starter.json").read_text())["id"] == "tos.example"
    finally:
        server.shutdown()
        server.server_close()
        thread.join()
print("CLI checks passed: production default, API override, anonymous/authenticated reads, first download.")
