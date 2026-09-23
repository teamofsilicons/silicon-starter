#!/bin/sh
# Starter build-hook environment.
# STARTER_SOURCE: absolute path to the directory containing starter.yaml.
# STARTER_OUTPUT: absolute path to this run's temporary output directory.
# STARTER_PROJECT: absolute destination path (may not exist on first pull).
# STARTER_INTERACTIVE: "true" for interactive setup, "false" for auto-updates.
# Auto-updates provide no terminal input; this sample never needs to prompt.
# Every active answer is exported as STARTER_VAR_<UPPERCASE_NAME>, including secrets.
# Inactive conditional answers are omitted. Collections are JSON strings.
# For example, STARTER_VAR_SILICON_TOKEN contains the supplied token.
# STARTER_VAR_WAVEFORM: validated boolean, exported as "true" or "false".
set -eu

mkdir -p "$STARTER_OUTPUT/workspace" "$STARTER_OUTPUT/memories"

# Conditional tool instructions are assembled by prompts/tools.md.tmpl.
# Each run starts with fresh output, so there is nothing to append or clean up.
# Creators can add commands here to modify the rendered files before merging.
