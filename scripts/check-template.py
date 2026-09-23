#!/usr/bin/env python3
"""End-to-end template lifecycle against an isolated registry; no production writes."""
import base64
import http.server
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import threading

binary = Path(sys.argv[1]).resolve()
sample = Path(__file__).resolve().parents[1] / "starter_template"
with tempfile.TemporaryDirectory(prefix="starter-template-check-") as directory:
    root = Path(directory)
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    env.update(HOME=str(root), SILICON_HOME=str(root), GIT_CONFIG_NOSYSTEM="1")
    env.pop("STARTER_API_URL", None)
    env["TZ"] = "UTC"

    def run(*args, cwd=root, ok=True):
        result = subprocess.run(args, cwd=cwd, env=env, text=True, capture_output=True)
        assert (result.returncode == 0) == ok, (args, result.stdout, result.stderr)
        return result.stdout if ok else result.stderr

    def git(*args, cwd):
        return run("git", "-c", "user.name=Test", "-c", "user.email=test@localhost", *args, cwd=cwd)

    author = root / "author"
    shutil.copytree(sample, author)
    git("init", "-b", "main", cwd=author)
    run(str(binary), "seed", "--check", cwd=author)
    assert not (author / "silicon.yaml").exists()
    run(str(binary), "seed", "--defaults", cwd=author)
    assert (author / "silicon.yaml").is_file()
    assert (author / "workspace").is_dir()
    assert (author / "memories").is_dir()
    assert "waveform" in (author / "prompts/tools.md").read_text().lower()
    assert (author / ".starterbase/.state/state.json").is_file()
    git("add", "-A", cwd=author)
    assert not git("ls-files", ".starterbase/.state", cwd=author).strip()
    git("commit", "-m", "Template", cwd=author)
    commit = git("rev-parse", "HEAD", cwd=author).strip()
    bundle = root / "source.bundle"
    git("bundle", "create", str(bundle), "main", cwd=author)
    uploads = []

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            assert self.path.startswith("/api/v1/starters/tos.sample/archive"), self.path
            self.reply({"commit": commit, "bundle_base64": base64.b64encode(bundle.read_bytes()).decode()})

        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            uploads.append((self.path, body))
            self.reply({"id": "tos.sample", "commit": body.get("commit")})

        def reply(self, body):
            encoded = json.dumps(body).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(encoded)))
            self.end_headers()
            self.wfile.write(encoded)

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    api = f"http://127.0.0.1:{server.server_port}"
    command = (str(binary), "--api", api)
    env["TZ"] = "Pacific/Honolulu"
    try:
        run(*command, "download", "tos.sample", "--dir", "instance", "--defaults", "--set", "purpose=Team research", "--set", "waveform=false")
        instance = root / "instance"
        state_file = instance / ".starterbase/.state/state.json"
        state = json.loads(state_file.read_text())
        assert state["answers"]["timezone"] == "Pacific/Honolulu", state["answers"]
        assert state["answers"]["waveform"] is False and state["answers"]["purpose"] == "Team research"
        assert "tos>waveform" not in (instance / "silicon.yaml").read_text()
        assert not (instance / "README.md").exists() and not (instance / "variables.yaml").exists()
        assert (instance / "workspace/.siliconkeep").is_file()
        assert not (instance / ".starterbase/.state/generated/workspace/.siliconkeep").exists()
        assert json.loads((instance / ".git/starter.json").read_text())["auto_update"] is True
        first = (instance / "silicon.yaml").read_bytes()
        run(*command, "seed", "--defaults", cwd=instance)
        assert (instance / "silicon.yaml").read_bytes() == first
        run(*command, "seed", "--defaults", "--set", "waveform=true", cwd=instance)
        assert "tos>waveform" in (instance / "silicon.yaml").read_text()
        assert "google" in (instance / "silicon.yaml").read_text()
        run(*command, "pull", "tos.sample", "--dir", "dev", "--defaults", "--set", "silicon_token=PRIVATE_INSTANCE_TOKEN")
        development = json.loads((root / "dev/.git/starter.json").read_text())
        assert development["mode"] == "development" and development["auto_update"] is False
        assert "PRIVATE_INSTANCE_TOKEN" not in git("show", "HEAD:silicon.yaml", cwd=root / "dev")
        run(*command, "download", "tos.sample", "--defaults", cwd=root / "dev", ok=False)
        run(*command, "pull", cwd=instance)
        assert json.loads((instance / ".git/starter.json").read_text())["mode"] == "download"
        run(*command, "update", "on", cwd=root / "dev", ok=False)
        occupied = run(*command, "pull", "tos.sample", "--dir", "dev", "--defaults", ok=False)
        assert "occupied" in occupied
        run(*command, "download", "tos.sample@1.0", "--dir", "pinned", "--defaults")
        assert json.loads((root / "pinned/.git/starter.json").read_text())["auto_update"] is False
        run(*command, "push", cwd=instance, ok=False)

        # An independent local edit survives an upstream template edit.
        tools = instance / "prompts/tools.md"
        tools.write_text("Local team guidance.\n\n" + tools.read_text())
        upstream = author / ".starterbase/prompts/silicon.md.tmpl"
        upstream.write_text(upstream.read_text() + "\nNew upstream instruction.\n")
        build = author / ".starterbase/build.sh"
        build.write_text(build.read_text() + '\nmkdir -p "$STARTER_OUTPUT/new-output"\n')
        git("add", "-A", cwd=author)
        git("commit", "-m", "New instruction", cwd=author)
        commit = git("rev-parse", "HEAD", cwd=author).strip()
        git("bundle", "create", str(bundle), "main", cwd=author)
        run(*command, "update", "off", cwd=instance)
        run(*command, "daemon", "--once")
        assert json.loads(state_file.read_text())["revision"] != commit
        recipe = instance / ".starterbase/starter.yaml"
        assert "auto_update: false" in recipe.read_text()
        recipe.write_text(recipe.read_text().replace("auto_update: false", "auto_update: true"))

        # Git history is part of the update: a failed commit must roll back too.
        git("add", "-A", cwd=instance)
        git("commit", "-m", "Local instance configuration", cwd=instance)
        old_head = git("rev-parse", "HEAD", cwd=instance)
        old_index = git("ls-files", "--stage", cwd=instance)
        old_state = state_file.read_bytes()
        baseline = instance / ".starterbase/.state/generated"
        old_baseline = {p.relative_to(baseline): p.read_bytes() for p in baseline.rglob("*") if p.is_file()}
        old_source = (instance / ".starterbase/prompts/silicon.md.tmpl").read_bytes()
        old_prompt = (instance / "prompts/silicon.md").read_bytes()
        hook = instance / ".git/hooks/pre-commit"
        hook.write_text("#!/bin/sh\nexit 1\n")
        hook.chmod(0o755)
        error = run(*command, "update", "now", cwd=instance, ok=False)
        assert "automatic updates are off" in error, error
        assert git("rev-parse", "HEAD", cwd=instance) == old_head
        assert git("ls-files", "--stage", cwd=instance) == old_index
        assert state_file.read_bytes() == old_state
        assert {p.relative_to(baseline): p.read_bytes() for p in baseline.rglob("*") if p.is_file()} == old_baseline
        assert (instance / ".starterbase/prompts/silicon.md.tmpl").read_bytes() == old_source
        assert (instance / "prompts/silicon.md").read_bytes() == old_prompt
        assert not (instance / "new-output").exists()
        hook.unlink()
        # Editing the recipe must work even while the older binding flag is off.
        recipe.write_text(recipe.read_text().replace("auto_update: false", "auto_update: true"))
        assert not git("status", "--porcelain", cwd=instance).strip()
        run(*command, "daemon", "--once")
        assert tools.read_text().startswith("Local team guidance.")
        assert "New upstream instruction." in (instance / "prompts/silicon.md").read_text()
        assert json.loads(state_file.read_text())["revision"] == commit
        assert not (instance / ".starterbase/.state/generated/prompts/tools.md").read_text().startswith("Local")
        assert (instance / "new-output/.siliconkeep").is_file()
        assert git("rev-list", "--count", old_head.strip() + "..HEAD", cwd=instance).strip() == "1"
        new_head = git("rev-parse", "HEAD", cwd=instance)
        run(*command, "update", "now", cwd=instance)
        assert git("rev-parse", "HEAD", cwd=instance) == new_head

        # Even a source-only revision gets a local update-history entry.
        (author / "README.md").write_text("Updated documentation only.\n")
        git("add", "README.md", cwd=author)
        git("commit", "-m", "Documentation revision", cwd=author)
        commit = git("rev-parse", "HEAD", cwd=author).strip()
        git("bundle", "create", str(bundle), "main", cwd=author)
        run(*command, "update", "now", cwd=instance)
        assert json.loads(state_file.read_text())["revision"] == commit
        assert git("rev-list", "--count", new_head.strip() + "..HEAD", cwd=instance).strip() == "1"

        # A conflicting generated edit leaves the entire installed revision alone.
        prompt = instance / "prompts/silicon.md"
        prompt.write_text(prompt.read_text().replace("New upstream instruction.", "Local replacement."))
        upstream.write_text(upstream.read_text().replace("New upstream instruction.", "Conflicting upstream replacement."))
        git("add", "-A", cwd=author)
        git("commit", "-m", "Conflict", cwd=author)
        commit = git("rev-parse", "HEAD", cwd=author).strip()
        git("bundle", "create", str(bundle), "main", cwd=author)
        before = prompt.read_bytes(), state_file.read_bytes()
        error = run(*command, "update", "now", cwd=instance, ok=False)
        assert "automatic updates are off" in error, error
        assert (prompt.read_bytes(), state_file.read_bytes()) == before
        assert not json.loads((instance / ".git/starter.json").read_text())["auto_update"]
        assert not git("ls-files", "-u", cwd=instance).strip()
        run(*command, "daemon", "--once")

        # Push compiles the recipe and uploads its default preview, not saved answers.
        (root / ".starter").mkdir(exist_ok=True)
        (root / ".starter/session").write_text("test-session")
        run(*command, "seed", "--defaults", "--set", "purpose=Personal answer", cwd=author)
        git("add", "-A", cwd=author)
        git("commit", "-m", "Personal working configuration", cwd=author)
        run(*command, "push", "tos.sample", cwd=author)
        assert uploads[-1][0] == "/api/v1/starters/tos.sample/push"
        uploaded = uploads[-1][1]
        pushed_state = json.loads((author / ".starterbase/.state/state.json").read_text())
        assert pushed_state["source_id"] == "tos.sample"
        assert pushed_state["revision"] == uploaded["commit"]
        preview_bundle = root / "preview.bundle"
        preview_bundle.write_bytes(base64.b64decode(uploaded["bundle_base64"]))
        preview = root / "preview"
        git("clone", "--branch", "main", str(preview_bundle), str(preview), cwd=root)
        assert "Personal answer" not in (preview / "prompts/silicon.md").read_text()
        assert not (preview / ".starterbase/.state").exists()
        run(*command, "seed", "--defaults", cwd=author)
        assert "Personal answer" in (author / "prompts/silicon.md").read_text()
    finally:
        server.shutdown()
        server.server_close()
        worker.join()
print("Template checks passed: compile, seed, install, saved answers, pins, developer isolation, generated merge, commit/conflict rollback, update history, recipe toggle, pause, default push preview.")
