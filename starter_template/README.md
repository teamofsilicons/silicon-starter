# Sample starter

This is a working authoring example for Starter recipes. Run `starter seed`
in this folder to generate a silicon, or `starter seed --check` to validate
its ingredients without executing commands.
It borrows the Waveform choice from `tos.classic`, but uses one ISI and a small
event route to keep the example readable.

Recipe expressions follow Stemcell's [reference YAML](https://github.com/teamofsilicons/silicon-stemcell/blob/main/stemcell/silicon/silicon.yaml)
and [rendering notes](https://github.com/teamofsilicons/silicon-stemcell/blob/main/UNDERSTANDING.md):
`!` for Bash, `{...}` for CEL, and `!>>` for fallbacks. File templates use
[Jinja-style syntax](https://jinja.palletsprojects.com/en/stable/templates/):
`{{ ... }}`, `{% if ... %}`, and `{% include ... %}`. Starter implements this format and its update lifecycle.

Start with [.starterbase/starter.yaml](.starterbase/starter.yaml). YAML defines
standard questions and templates; [build.sh](.starterbase/build.sh) gives the
creator a full local script. Scripts run with the user's trust, active answers,
and terminal access during interactive setup. They can read files, ask
follow-up questions, run tools, call APIs, and perform whatever setup the
starter needs. Automatic updates run without questions or terminal input.

## Before: running from testing/

Assume this sample has been published as `tos.classic-sample`. Your terminal's
current directory is an empty `testing/` folder:

```text
testing/                            current working directory; empty
```

## Pull, answer the questions, and stop

An interactive developer pull looks like this:

```text
testing $ starter pull tos.classic-sample

Pulling tos.classic-sample...

Folder name: research-assistant
What is this silicon's ID? [empty] research:tos
What is this silicon's token? [empty] ********
What should this silicon focus on?
  [Help the team research, plan, and finish its work.] Research for the team.
Include Waveform for voice messages? [yes] yes
Preparing TTS provider choices...
Which default TTS provider would you like? [google] <Enter>

Building research-assistant...
Created ./research-assistant
Auto-update: disabled (developer checkout; use starter download for an updating instance)

testing $
```

The command finishes here. This sample creates the configured project and
returns to the shell in `testing/`; it does not start the silicon or change
your terminal's working directory.

Every recipe question can be skipped with Enter to accept its resolved
default. The timezone has no `prompt`, so it resolves silently; this example
assumes its command detects `Asia/Kolkata`.
The ID and token default to empty strings: the sample can generate files
without them, but those defaults do not create an identity or working login.
Creators can supply command defaults to resolve those values in their setup.

The folder question belongs to Starter and appears for every new instance
unless a destination was already supplied. An occupied name needs a different
answer; it never means updating that folder. Folder name and silicon identity
are separate. Pulling the same starter again can create another named folder
alongside `research-assistant/` with its own answers and update state.

## After: the configured project inside testing/

With the answers above, the generated file layout is below. Git internals are
omitted so the source ingredients and generated files are easy to see.

```text
testing/
  research-assistant/
    silicon.yaml                    rendered with your ID, token, and timezone
    prompts/
      silicon.md                    rendered with your chosen focus
      tools.md                      common instructions plus Waveform section
    workspace/                      empty, ready for work
    memories/                       empty, ready for notes
    .starterbase/                   original ingredients, retained for reruns
      starter.yaml
      build.sh
      waveform-providers.sh
      silicon.yaml.tmpl
      prompts/
        silicon.md.tmpl
        tools.md.tmpl
      optional/
        waveform.md
      .gitignore
      .state/                       created by Starter, not the author
        state.json                  source ID, exact revision, all answers
        generated/                  pure generated baseline, before local merges
          silicon.yaml
          prompts/
            silicon.md
            tools.md
```

The installed `silicon.yaml` includes `prompts/tools.md` in DNA regardless of
the Waveform answer. With `waveform: true`, that file contains the Waveform
section, the app list contains `tos>waveform`, and its app config sets
`default_tts_provider: google`. With `false`, the section, app entry, and app
config are absent. The source fragment in `.starterbase/optional/`
remains available for later reconfiguration; there is no separate installed
Waveform prompt file.

The sample's walkthrough README and variable catalog are not installed. All
source ingredients live under `.starterbase/`, and declared or script-generated outputs become runtime
files. Starter retains the ingredients for reruns. Runtime DNA should use the explicit generated paths shown in `silicon.yaml`;
`.starterbase/` contains authoring ingredients and private generation state.

## Automatic updates

The creator sets a top-level boolean in `starter.yaml`, separate from questions:

```yaml
auto_update: true
```

`true` enables unattended builds and merges when the updater finds a new
published revision. `false` disables those automatic actions; explicit updates
and reconfiguration remain available. If omitted, it defaults to `false`.

The creator's value initializes a downloaded instance. Developer pulls never
auto-update, regardless of this recipe preference. After installation, the local
`.starterbase/starter.yaml` controls that instance. An upgrade preserves its
current `auto_update` value when replacing the recipe, so a local `false` is
not reset by a creator's `true`. Saved answers and mandatory defaults let an
enabled instance generate the next version without asking questions.

## Optional answers, required defaults

Every variable declares a `default`; there is no `required` field. A missing
default or a default with the wrong type makes the recipe invalid.
Questions remain useful for interactive setup; answering them is optional.
The `prompt` field itself is optional: omit it for a variable that never asks
a question, whether its default is a literal or an expression.

[variables.yaml](variables.yaml) shows the complete type catalog:

| Type | Value | Example default |
| --- | --- | --- |
| `string` | Text, including multiline text | `Research assistant` |
| `secret` | Text with masked terminal input | `""` |
| `boolean` | Yes/no, stored as a boolean | `true` |
| `number` | A whole number, including zero or negatives | `1` |
| `float` | A finite number, including fractions | `1.0` |
| `select` | One string from `choices` | `google` |
| `multiselect` | A list of distinct strings from `choices` | `[monday, friday]` |
| `list` | A free-form JSON array | `[research, planning]` |
| `object` | A JSON object, with nesting allowed | `{tone: concise}` |

These are the two numeric types: `number` rejects fractional values instead
of truncating them; `float` accepts whole or fractional values but rejects
NaN and infinity. There is no separate `integer` type or alias.

Lists and objects accept JSON during terminal input; their YAML defaults use
native lists and mappings. Structured values stay structured in saved answers
and the template context. A select answer must appear in its choices;
a multiselect answer must be a subset (an empty list is allowed). Check defaults
when they are resolved; do not evaluate an unused dynamic default just to check it.
Paths, URLs, email addresses, and dates are strings. Input widgets and extra
validation can be added separately; they do not need more storage types.
Computed defaults and conditional questions work with these same types.

Defaults can be literal values, such as `true` or an empty string, or
Stemcell-style expressions:

```yaml
timezone:
  type: string
  default: ! node -p 'Intl.DateTimeFormat().resolvedOptions().timeZone' !>> "UTC"
```

This asks Node for the local IANA timezone. If the command is unavailable or
fails, `!>> "UTC"` supplies the default. Following Stemcell, fallbacks activate
on evaluation errors or nonzero command exits. Successful empty output and
`false` are values, not failures. A command that considers empty output invalid
should exit nonzero. Dynamic defaults must end in a literal fallback of the
declared type, so unattended setup has a final value to use.

Default expressions use the recipe directory as their working directory and
run without interactive input. CEL expands first, an explicit `!` runs Bash,
and the result becomes the answer. Shell stdout has trailing CR/LF removed;
stderr is diagnostic output. Declared variable types control conversion:
strings, secrets, and selections stay strings; booleans must resolve to `true`
or `false`; numbers must parse as the declared numeric type. Lists, objects,
and multiselect answers use JSON output. An invalid result stops the update
without prompting.

Resolution order is an explicitly supplied answer, then a saved answer, then
the default. A saved `false` or empty string counts as an answer. Evaluate a
default command only when a default is needed, and save its resolved value
after a successful apply. An update does not rerun a timezone lookup or reset
an existing answer just because the creator changed the default. Interactive
reconfiguration can explicitly choose to resolve the current default again.
The CEL namespace `var` contains active resolved answers, including secrets.
Resolve variables in declaration order so a default may refer to earlier
answers. Forward references and dependency cycles are invalid.
Reading the recipe or validating expression syntax must not run its commands.

Variables without questions use the same rules. For example:

```yaml
summary_heading:
  type: string
  default: Team summary

introduction:
  type: string
  default: '{var.display_name + " is here to help."} !>> "Here to help."'
```

Here `display_name` is an earlier variable, as shown in the catalog. Both
values resolve without terminal questions and are saved in `state.json`,
available under `var`, and exported to scripts. Omitting `prompt` changes only
the interaction: explicit values still override saved values, then defaults.
An expression in `default` is evaluated only when a value is missing or the
default is explicitly reset. Changing `display_name` later does not recalculate
a saved `introduction`. These are saved defaults, not live formulas.

## Follow-up questions and preparation

The main recipe asks about Waveform first, then its default TTS provider:

```yaml
waveform:
  type: boolean
  prompt: Include Waveform for voice messages?
  default: true

waveform_tts_provider:
  type: select
  when: '{var.waveform}'
  prompt: Which default TTS provider would you like?
  choices: ! sh waveform-providers.sh !>> ["google"]
  default: google
```

Google is a **provider** in Stemcell's Waveform config. The template writes the
answer into `silicon.app_configs.tos>waveform.default_tts_provider`. A recipe
could add a model question after that, depending on the selected provider.

For each variable, the sequence is: evaluate `when` (omitted means true),
prepare `choices` if present, resolve the explicit/saved/default value, then
offer the question during interactive setup only if `prompt` is present.
Validate the resulting value before moving to the next variable.

When present, `when` is an expression that returns a boolean, such as
`'{var.waveform}'` or `'{var.daily_summary_count > 0}'`. Omission is equivalent
to `'{true}'`. Evaluate it on every run, even for saved values or variables
without prompts. The result must be a boolean, not a truthy string or number:
the string `"false"` is invalid. A nonboolean result or expression error stops
the build instead of silently hiding a variable.

With Waveform disabled, skip the provider question **and** its choices/default
commands. Keep any previously saved provider answer in `state.json` for later
reenabling, but omit inactive answers from `var` and script environment
variables. Templates therefore access the provider only inside the Waveform
condition. A never-enabled variable has no saved value until it becomes active.

`choices` accepts a literal list of strings or a Stemcell-style expression.
Its command is the processing step before the question: it can read files,
inspect earlier answers, call APIs, or run an author-supplied script. It returns
a JSON array on stdout; stderr carries diagnostics. The sample's
[waveform-providers.sh](.starterbase/waveform-providers.sh) returns `["google"]`
as an illustrative catalog; it does not pretend to discover real providers.
Creators replace its body with their own discovery. Dynamic choices must end
in a literal fallback list, just as dynamic defaults need a literal fallback.
Malformed output or a resolved default outside the choices is an error.

Preparation commands receive earlier active answers as `STARTER_VAR_*`, plus
`STARTER_SOURCE` and `STARTER_PROJECT`. Like default commands, they run in the
recipe directory without terminal input (`STARTER_INTERACTIVE=false`), even
during interactive setup. `STARTER_OUTPUT` is available later, for the build.
This lets the same preparation run unattended without another hook type.

On updates, resolve choices for active variables and validate saved answers
against them. Reuse a valid saved choice; use the default for a newly active
variable without a value. If a saved choice has disappeared, interactive
setup can offer a replacement when the variable has a prompt. Otherwise, or
during an unattended update, defer for attention and an explicit replacement.
It never silently replaces a saved choice with a new default. Answers and
source state are committed only after the complete build and merge succeed.

## Rendering and script contract

1. Resolve YAML variables in declaration order, checking conditions, preparing
   choices, and validating active values. Interactive setup offers questions
   only for variables with a `prompt`;
   automatic updates resolve saved values and defaults without prompts.
2. Render `.tmpl` sources as Jinja-style text into a fresh temporary output
   directory, making parent folders as needed. Other declared files copy
   verbatim. YAML validation follows the build commands.
3. Run the ordered `build` expressions with the recipe directory as the working
   directory. Each `!` command uses Bash and the same `!>>` fallback rules.
   Attach terminal input only during interactive setup.
4. Validate the resulting configuration and apply the generated files. On a
   rerun, compare against the previous build and current project first.
5. After a successful apply, save the answers, applied source revision, and
   pure generated snapshot from before the merge. Retain the ingredients and
   the instance's `auto_update` setting; discard temporary output.

Starter's validation checks generated YAML structure and known output
references. It permits the sample's intentionally empty ID/token defaults;
Stemcell checks runtime readiness when the user connects the silicon. This
generation check leaves runtime expressions deferred. Stemcell's `compile`
command evaluates compile-time commands such as `SILICON_HOME`, so it is not
the validation step described here.

Build commands use the same compact style as Stemcell's setup commands:

```yaml
build:
  - ! sh build.sh
```

Build commands run only after answers are resolved and files rendered. A failed
build command without a successful fallback stops the build. Commands can
modify rendered files in `STARTER_OUTPUT` before validation and application.

## Jinja-style templates and conditional tools

The template context contains active resolved answers under `var`. For example,
`{{ var.purpose }}` inserts the chosen focus. A `.tmpl` file is rendered once;
answers are data and their contents are not interpreted again as templates.

[prompts/tools.md.tmpl](.starterbase/prompts/tools.md.tmpl) assembles one prompt:

```jinja
# Tools

Use `dm --help` to learn how to message the team.
Use `briefcase --help` to learn how to share files with the right access.

{% if var.waveform %}
{% include "optional/waveform.md" %}
{% endif %}
```

The include loads the source fragment relative to `.starterbase/` and renders
it with the same context. It can itself use variables and conditionals. An
include contributes text to the current output file; it does not install a
separate file or read a previous generated `tools.md`. Both conditions and
includes are reevaluated on each build from the current saved answers.

Use normal Jinja conditionals, loops, includes, and comments. For this
renderer, undefined variables fail the build, HTML autoescaping is disabled,
and trailing newlines are retained. Set `trim_blocks` and `lstrip_blocks` so
control lines do not add empty lines or disturb YAML indentation. Literal
Jinja examples can be wrapped in `{% raw %}` / `{% endraw %}`.

For YAML scalars, `tojson` provides a quoted JSON value that is also valid YAML:

```jinja
silicon:
  id: {{ var.silicon_id | tojson }}
  timezone: {{ var.timezone | tojson }}
  SILICON_HOME: ! pwd
  apps:
    - tos>dm
    - tos>briefcase
{% if var.waveform %}
    - tos>waveform

  app_configs:
    tos>waveform:
      default_tts_provider: {{ var.waveform_tts_provider | tojson }}
{% endif %}
```

Do not add another pair of quotes around `tojson`. This handles quotes and
newlines in answers without needing a custom YAML filter.

Stemcell's single-brace runtime expressions and `!` commands are plain text
to Jinja. The template can contain `message: '{make_readable(request.tings)}'`
and `SILICON_HOME: ! pwd` directly. Starter does not evaluate them while
rendering files. The generated batch route and runtime commands are evaluated
later by Stemcell. Recipe `when`, `choices`, `default`, and `build` expressions
use the Stemcell-style evaluator during setup.

Bare `!` in recipe or generated YAML still needs Stemcell's parser convention;
an ordinary YAML loader may treat it as a tag. Inside recipe expressions,
`$NAME` avoids the CEL brace ambiguity of shell `${NAME}`.

The build script receives:

| Variable | Value |
| --- | --- |
| `STARTER_SOURCE` | Absolute directory containing the recipe and ingredients. |
| `STARTER_OUTPUT` | Absolute temporary output directory for this build. |
| `STARTER_PROJECT` | Absolute destination, such as `/path/to/testing/research-assistant`; it may not exist on first pull. |
| `STARTER_INTERACTIVE` | `true` for interactive setup; `false` for automatic updates and default commands. |
| `STARTER_VAR_<UPPERCASE_NAME>` | Every active answer, including secrets; booleans use `true` or `false`, numbers use numeric text, and lists/objects/multiselects use JSON. |

For example, `$STARTER_VAR_SILICON_TOKEN` contains the token supplied above,
and `$STARTER_VAR_WAVEFORM` is `true`. `type: secret` masks terminal input;
the value otherwise behaves like a string. It is available during rendering,
passed to scripts, saved with the local answers, and present in rendered files
and the generation snapshot. All rendering happens before the script runs,
so the script sees the complete configuration. `.starterbase/.state/` is
ignored by Git.

The script can ask additional questions when `STARTER_INTERACTIVE=true`.
YAML-declared answers are saved and reused automatically; the creator handles
persistence for extra answers gathered directly by their script. With
`STARTER_INTERACTIVE=false`, stdin is closed and scripts must use saved inputs
or their own defaults. If they cannot proceed, they exit with an error instead
of prompting. Scripts remain trusted local code; this flag is their contract
for supporting unattended updates.

`STARTER_OUTPUT` is how scripts give files to Starter's update and merge flow.
The sample uses it to create the workspace and memories directories. A script
can also inspect or modify the installed project, install dependencies, and
provision services.
Those direct changes are managed by the script itself; staging and file merges
only manage output written into `STARTER_OUTPUT`. The script must exit
successfully before Starter applies that output.

## Reconfiguring and updating

Reconfigure rebuilds the installed recipe revision with changed answers.
Update fetches a new revision into staging and rebuilds it with saved answers.
New active variables resolve their defaults automatically. A newly added
boolean with `default: true` is enabled for existing instances without asking;
an existing saved `false` stays false. Adding the TTS provider question upgrades
instances with Waveform enabled using `google`; instances with Waveform disabled
skip it until enabled. Publishing makes the update available;
instances with automatic updates enabled apply it through this same flow.

Source ID and exact revision are recorded by Starter from the download, not
manually declared by the creator. `schema: 1` describes this recipe
format, not the starter's release number.

## When the silicon edits tools.md

The silicon can edit `prompts/tools.md` normally. Those edits belong to the
instance and are preserved separately from the creator's generated baseline.
When the creator changes either the main tools template or its Waveform
fragment, Starter renders the new recipe with this instance's answers and
performs a three-way merge of the generated files:

```text
BASE   .starterbase/.state/generated/prompts/tools.md
LOCAL  prompts/tools.md (including the silicon's edits)
NEW    the new recipe's rendered prompts/tools.md in temporary output
```

For example, the silicon adds a team-specific DM instruction while the creator
updates the Waveform section. Independent edits can merge into one `tools.md`
containing both changes. If both rewrite the same voice instruction, Starter
leaves the installed files intact, reports the conflict outside runtime files,
and disables automatic updates.

| Change | Result |
| --- | --- |
| Only the creator changes a region | Apply that change. |
| Only the silicon changes a region | Keep the local change. |
| Both make independent edits | Merge both when the changes reconcile cleanly. |
| Both change the same region incompatibly | Defer the update for review. |
| Waveform is false and only its fragment changes | Update retained ingredients; leave live `tools.md` unchanged. |

The merge works on rendered text, so includes and Jinja blocks do not become
special ownership boundaries inside the installed file. Keep the pure NEW
output as the next `.state/generated/` baseline after success, never the merged
live file. Otherwise the silicon's customizations would be mistaken for
creator content on the next update. The baseline includes build-hook output
but excludes changes retained from the local project during merging.

Disabling Waveform renders a `tools.md` without its section and removes the app
entry from `silicon.yaml`. An untouched old section can be removed; a locally
edited section may conflict. The entire staged update waits on any conflict,
including the app/config changes, so half of the update is never applied.
Keep the live files intact and report conflicts outside runtime prompts.
Validate the merged result before applying it. Files Starter never generated
remain untouched; a new generated file colliding with an existing local file
also requires reconciliation. Remove an obsolete generated file only if it
is untouched; preserve a locally modified file and defer for review.

Directories such as `workspace/` and `memories/` are created if absent; their
user-created contents never become owned outputs. Missing `.state` after an
export or fresh Git clone means there is no generation baseline: Starter must
recover that state or request reconciliation before updating existing files.

Automatic updates never open questions or wait for conflict resolution. A
failed build, missing baseline, invalid result, or unresolved conflict leaves
the staged update unapplied and is reported for later attention. Starter does
not replace the installed recipe or advance its saved revision. Defaults let
new variables resolve unattended; they do not remove merge conflicts. Starter
also does not undo commands or direct changes already performed by scripts;
the creator defines how those actions behave when repeated.
