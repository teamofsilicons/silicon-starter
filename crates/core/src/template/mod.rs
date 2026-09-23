//! Trusted, local starter recipes. Parsing/compilation never execute recipe commands.
mod expr;
mod jinja;
mod yaml;

use indexmap::IndexMap;
use minijinja::{AutoEscape, Environment, UndefinedBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    collections::HashSet,
    fs,
    io::{self, Write},
    path::{Component, Path, PathBuf},
};

type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub schema: u32,
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub auto_update: bool,
    #[serde(default)]
    pub variables: IndexMap<String, Variable>,
    #[serde(default)]
    pub files: Vec<FileSpec>,
    #[serde(default)]
    pub build: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Variable {
    #[serde(rename = "type")]
    pub kind: VariableType,
    pub prompt: Option<String>,
    pub default: Value,
    pub when: Option<String>,
    pub choices: Option<Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VariableType {
    String,
    Secret,
    Boolean,
    Number,
    Float,
    Select,
    Multiselect,
    List,
    Object,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileSpec {
    pub from: String,
    pub to: String,
}

#[derive(Debug)]
pub struct BuildResult {
    /// Contains inactive saved answers too; only active answers are passed to templates/scripts.
    pub answers: Map<String, Value>,
    pub auto_update: bool,
}

pub fn parse_yaml(text: &str) -> Result<serde_yaml::Value> {
    serde_yaml::from_str(&yaml::prepare_scalars(text)?).map_err(|e| format!("invalid YAML: {e}"))
}

/// Safe for recipe previews: structural, expression, type and dependency checks only.
pub fn parse_recipe(text: &str) -> Result<Recipe> {
    let recipe: Recipe = serde_yaml::from_value(parse_yaml(text)?)
        .map_err(|e| format!("invalid starter recipe: {e}"))?;
    if recipe.schema != 1 {
        return Err(format!(
            "unsupported recipe schema {}; expected 1",
            recipe.schema
        ));
    }
    let mut known = HashSet::new();
    let mut environment_names = HashSet::new();
    for (name, var) in &recipe.variables {
        if name.is_empty()
            || !name
                .bytes()
                .enumerate()
                .all(|(i, c)| c == b'_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()))
        {
            return Err(format!(
                "invalid variable name `{name}`; use letters, digits and underscores"
            ));
        }
        if !environment_names.insert(name.to_ascii_uppercase()) {
            return Err(format!(
                "variable `{name}` collides with another script environment name"
            ));
        }
        if let Some(when) = &var.when {
            if !expr::dynamic(&Value::String(when.clone())) {
                return Err(format!("{name}.when must be a boolean expression"));
            }
            expr::validate(when, &known).map_err(|e| format!("{name}.when: {e}"))?;
        }
        validate_default(&var.default, var.kind, &known)
            .map_err(|e| format!("{name}.default: {e}"))?;
        let selection = matches!(var.kind, VariableType::Select | VariableType::Multiselect);
        if selection != var.choices.is_some() {
            return Err(format!(
                "{name}: choices must be present exactly for select/multiselect variables"
            ));
        }
        if let Some(choices) = &var.choices {
            let literal_choices = if expr::dynamic(choices) {
                validate_default(choices, VariableType::List, &known)?;
                expr::literal(expr::candidates(choices.as_str().unwrap())?.last().unwrap())?
            } else {
                choices.clone()
            };
            validate_choices(&literal_choices).map_err(|e| format!("{name}.choices: {e}"))?;
            // Dynamic choices may legitimately differ from their fallback; check membership at resolution.
            if !expr::dynamic(choices) && !expr::dynamic(&var.default) {
                validate_value(&var.default, var.kind, Some(&literal_choices))
                    .map_err(|e| format!("{name}.default: {e}"))?;
            }
        }
        known.insert(name.clone());
    }
    let mut destinations = HashSet::new();
    for file in &recipe.files {
        safe_relative(&file.from)?;
        let path = safe_relative(&file.to)?
            .to_string_lossy()
            .to_ascii_lowercase();
        let path = Path::new(&path);
        if !destinations.insert(path.to_owned()) {
            return Err(format!("duplicate output {}", file.to));
        }
        if destinations
            .iter()
            .any(|other| other != path && (other.starts_with(path) || path.starts_with(other)))
        {
            return Err(format!("overlapping output {}", file.to));
        }
    }
    for command in &recipe.build {
        expr::validate(command, &known)?;
    }
    Ok(recipe)
}

fn validate_default(value: &Value, kind: VariableType, known: &HashSet<String>) -> Result<()> {
    if expr::dynamic(value) {
        let source = value.as_str().unwrap();
        let candidates = expr::candidates(source)?;
        if candidates.len() < 2 {
            return Err("dynamic values must end in a literal !>> fallback".into());
        }
        let fallback = expr::literal(candidates.last().unwrap())?;
        validate_value(&fallback, kind, None)?;
        expr::validate(source, known)
    } else {
        validate_value(value, kind, None)
    }
}

fn validate_choices(value: &Value) -> Result<()> {
    let values = value
        .as_array()
        .ok_or("choices must be a list of strings")?;
    let mut seen = HashSet::new();
    for item in values {
        let item = item.as_str().ok_or("choices must contain only strings")?;
        if !seen.insert(item) {
            return Err("choices must be distinct".into());
        }
    }
    Ok(())
}

fn validate_value(value: &Value, kind: VariableType, choices: Option<&Value>) -> Result<()> {
    let valid = match kind {
        VariableType::String | VariableType::Secret | VariableType::Select => value.is_string(),
        VariableType::Boolean => value.is_boolean(),
        VariableType::Number => value.as_i64().is_some() || value.as_u64().is_some(),
        VariableType::Float => value.as_f64().is_some_and(f64::is_finite),
        VariableType::List => value.is_array(),
        VariableType::Object => value.is_object(),
        VariableType::Multiselect => value.as_array().is_some_and(|v| {
            let mut seen = HashSet::new();
            v.iter()
                .all(|item| item.as_str().is_some_and(|s| seen.insert(s)))
        }),
    };
    if !valid {
        return Err(format!(
            "value must have declared type {}",
            serde_json::to_value(kind).unwrap().as_str().unwrap()
        ));
    }
    if let Some(choices) = choices {
        let choices = choices.as_array().ok_or("choices must be a list")?;
        if kind == VariableType::Select && !choices.contains(value) {
            return Err(
                "answer is not in the available choices; supply an explicit replacement".into(),
            );
        }
        if kind == VariableType::Multiselect
            && value
                .as_array()
                .unwrap()
                .iter()
                .any(|v| !choices.contains(v))
        {
            return Err(
                "answer contains unavailable choices; supply an explicit replacement".into(),
            );
        }
    }
    Ok(())
}

fn convert(value: Value, kind: VariableType) -> Result<Value> {
    if let Value::String(text) = &value
        && !matches!(
            kind,
            VariableType::String | VariableType::Secret | VariableType::Select
        )
    {
        return serde_json::from_str(text)
            .map_err(|_| format!("expression output is not valid {:?} JSON", kind));
    }
    Ok(value)
}

fn resolve(value: &Value, kind: VariableType, runtime: &expr::Runtime<'_>) -> Result<Value> {
    if expr::dynamic(value) {
        convert(runtime.evaluate(value.as_str().unwrap())?, kind)
    } else {
        Ok(value.clone())
    }
}

/// Only relative project paths are managed. Internal state and Git can never be output.
fn safe_relative(name: &str) -> Result<&Path> {
    let path = Path::new(name);
    if name.is_empty() || name.contains('\\') || !path.components().all(|c| matches!(c,Component::Normal(_))) || path.components().any(|c| matches!(c,Component::Normal(v) if [".git", ".starterbase", ".state"].iter().any(|reserved| v.to_string_lossy().eq_ignore_ascii_case(reserved)))) {
        return Err(format!("unsafe recipe path `{name}`"));
    }
    Ok(path)
}

fn source_file(source: &Path, name: &str) -> Result<PathBuf> {
    let path = source.join(safe_relative(name)?);
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("cannot read recipe source {name}: {e}"))?;
    if !canonical.starts_with(source) || !canonical.is_file() {
        return Err(format!(
            "recipe source escapes its directory or is not a file: {name}"
        ));
    }
    Ok(canonical)
}

fn environment(source: &Path) -> Environment<'static> {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    env.set_auto_escape_callback(|_| AutoEscape::None);
    env.set_keep_trailing_newline(true);
    env.set_trim_blocks(true);
    env.set_lstrip_blocks(true);
    env.set_recursion_limit(100);
    env.set_fuel(Some(10_000_000));
    let root = source.to_owned();
    env.set_loader(move |name| {
        let path = source_file(&root, name)
            .map_err(|e| minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, e))?;
        fs::read_to_string(&path).map(Some).map_err(|e| {
            minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, e.to_string())
        })
    });
    env
}

fn walk(root: &Path, dir: &Path, output: bool) -> Result<Vec<PathBuf>> {
    let mut result = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !output && entry.file_name() == ".state" {
            continue;
        }
        let path = entry.path();
        let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        let relative = path.strip_prefix(root).unwrap();
        if output {
            safe_relative(relative.to_str().ok_or("output paths must be UTF-8")?)?;
        }
        if meta.file_type().is_symlink() {
            return Err(format!(
                "symlinks are not supported in recipe sources or generated output: {}",
                relative.display()
            ));
        }
        if meta.is_dir() {
            result.extend(walk(root, &path, output)?);
        } else if !meta.is_file() {
            return Err(format!(
                "unsupported filesystem entry {}",
                relative.display()
            ));
        }
        result.push(path);
    }
    Ok(result)
}

pub fn compile(source: &Path) -> Result<Recipe> {
    let source = source
        .canonicalize()
        .map_err(|e| format!("cannot open recipe source: {e}"))?;
    walk(&source, &source, false)?;
    let recipe = parse_recipe(
        &fs::read_to_string(source.join("starter.yaml"))
            .map_err(|e| format!("cannot read starter.yaml: {e}"))?,
    )?;
    let env = environment(&source);
    let mut checked = HashSet::new();
    for file in &recipe.files {
        let path = source_file(&source, &file.from)?;
        if path.extension().is_some_and(|ext| ext == "tmpl") {
            jinja::validate(&source, &env, &file.from, &recipe, &mut checked)?;
        }
    }
    Ok(recipe)
}

pub fn auto_update(source: &Path) -> Result<bool> {
    Ok(
        parse_recipe(&fs::read_to_string(source.join("starter.yaml")).map_err(|e| e.to_string())?)?
            .auto_update,
    )
}

/// Preserve commands and every other YAML field when saving an instance preference.
pub fn set_auto_update(source: &Path, value: bool) -> Result<()> {
    let path = source.join("starter.yaml");
    let mut document = parse_yaml(&fs::read_to_string(&path).map_err(|e| e.to_string())?)?;
    document
        .as_mapping_mut()
        .ok_or("recipe must be a mapping")?
        .insert(
            serde_yaml::Value::String("auto_update".into()),
            serde_yaml::Value::Bool(value),
        );
    let text = serde_yaml::to_string(&document).map_err(|e| e.to_string())?;
    parse_recipe(&text)?;
    fs::write(&path, text).map_err(|e| e.to_string())
}

fn question(
    name: &str,
    var: &Variable,
    initial: Option<Value>,
    choices: Option<&Value>,
) -> Result<Value> {
    let label = var.prompt.as_deref().unwrap_or(name);
    if let Some(choices) = choices {
        eprintln!("Choices: {}", expr::text(choices));
    }
    loop {
        let current = initial
            .as_ref()
            .map(|v| {
                if var.kind == VariableType::Secret {
                    "hidden".into()
                } else {
                    expr::text(v)
                }
            })
            .unwrap_or_else(|| "replacement required".into());
        let prompt = format!("{label} [{current}]: ");
        let input = if var.kind == VariableType::Secret {
            rpassword::prompt_password(prompt)
                .map_err(|e| format!("cannot read secret input: {e}"))?
        } else {
            eprint!("{prompt}");
            io::stderr().flush().map_err(|e| e.to_string())?;
            let mut input = String::new();
            if io::stdin()
                .read_line(&mut input)
                .map_err(|e| e.to_string())?
                == 0
            {
                return Err("terminal input closed before answers were resolved".into());
            }
            input.trim_end_matches(['\r', '\n']).to_owned()
        };
        let answer = if input.is_empty() {
            if let Some(value) = &initial {
                value.clone()
            } else {
                eprintln!("Supply a valid replacement.");
                continue;
            }
        } else {
            let input = if var.kind == VariableType::Boolean {
                match input.to_ascii_lowercase().as_str() {
                    "yes" | "y" => "true".into(),
                    "no" | "n" => "false".into(),
                    _ => input,
                }
            } else {
                input
            };
            match convert(Value::String(input), var.kind) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("{e}");
                    continue;
                }
            }
        };
        match validate_value(&answer, var.kind, choices) {
            Ok(()) => return Ok(answer),
            Err(e) => eprintln!("{e}"),
        }
    }
}

pub fn build(
    source: &Path,
    project: &Path,
    output: &Path,
    explicit: &Map<String, Value>,
    saved: &Map<String, Value>,
    interactive: bool,
) -> Result<BuildResult> {
    let source = source.canonicalize().map_err(|e| e.to_string())?;
    let project = std::path::absolute(project).map_err(|e| e.to_string())?;
    let recipe = compile(&source)?;
    for key in explicit.keys() {
        if !recipe.variables.contains_key(key) {
            return Err(format!("unknown explicit variable `{key}`"));
        }
    }
    if output.exists()
        && fs::read_dir(output)
            .map_err(|e| e.to_string())?
            .next()
            .is_some()
    {
        return Err("build output directory must be empty".into());
    }
    fs::create_dir_all(output).map_err(|e| e.to_string())?;
    let output = output.canonicalize().map_err(|e| e.to_string())?;
    let mut active = Map::new();
    let mut answers = saved.clone();
    for (name, var) in &recipe.variables {
        let runtime = expr::Runtime {
            source: &source,
            project: &project,
            output: None,
            interactive: false,
            active: &active,
        };
        if let Some(when) = &var.when {
            let enabled = runtime
                .evaluate(when)
                .map_err(|e| format!("{name}.when: {e}"))?;
            if !enabled
                .as_bool()
                .ok_or_else(|| format!("{name}.when must return a boolean"))?
            {
                continue;
            }
        }
        let choices = var
            .choices
            .as_ref()
            .map(|value| resolve(value, VariableType::List, &runtime))
            .transpose()
            .map_err(|e| format!("{name}.choices: {e}"))?;
        if let Some(choices) = &choices {
            validate_choices(choices).map_err(|e| format!("{name}.choices: {e}"))?;
        }
        let value = match explicit.get(name).or_else(|| saved.get(name)) {
            Some(value) => value.clone(),
            None => resolve(&var.default, var.kind, &runtime)
                .map_err(|e| format!("{name}.default: {e}"))?,
        };
        let validation = validate_value(&value, var.kind, choices.as_ref());
        if !saved.contains_key(name) && !explicit.contains_key(name) {
            validation
                .clone()
                .map_err(|e| format!("{name}.default: {e}"))?;
        }
        let value = if interactive && var.prompt.is_some() && !explicit.contains_key(name) {
            // A vanished saved selection requires an explicit replacement, not the new default.
            question(
                name,
                var,
                validation.is_ok().then_some(value),
                choices.as_ref(),
            )?
        } else {
            validation.map_err(|e| format!("{name}: {e}"))?;
            value
        };
        active.insert(name.clone(), value.clone());
        answers.insert(name.clone(), value);
    }
    let env = environment(&source);
    for file in &recipe.files {
        let input = source_file(&source, &file.from)?;
        let destination = output.join(safe_relative(&file.to)?);
        fs::create_dir_all(destination.parent().unwrap()).map_err(|e| e.to_string())?;
        if input.extension().is_some_and(|ext| ext == "tmpl") {
            let rendered = env
                .get_template(&file.from)
                .and_then(|template| template.render(minijinja::context! {var => &active}))
                .map_err(|e| format!("cannot render {}: {e}", file.from))?;
            fs::write(&destination, rendered).map_err(|e| e.to_string())?;
        } else {
            fs::copy(input, &destination).map_err(|e| e.to_string())?;
        }
    }
    let runtime = expr::Runtime {
        source: &source,
        project: &project,
        output: Some(&output),
        interactive,
        active: &active,
    };
    for command in &recipe.build {
        runtime.evaluate(command)?;
    }
    validate_output(&output)?;
    Ok(BuildResult {
        answers,
        auto_update: recipe.auto_update,
    })
}

pub fn validate_output(output: &Path) -> Result<()> {
    validate_merged_output(output, output, &[])
}

/// Check a merged generated candidate while allowing references to user-owned project files.
pub fn validate_merged_output(output: &Path, project: &Path, removed: &[String]) -> Result<()> {
    walk(output, output, true)?;
    let text = fs::read_to_string(output.join("silicon.yaml"))
        .map_err(|e| format!("build must produce silicon.yaml: {e}"))?;
    let yaml = parse_yaml(&text)?;
    let normalized = serde_yaml::to_string(&yaml).map_err(|e| e.to_string())?;
    crate::validate_silicon_yaml(&normalized)?;
    let map = yaml.as_mapping().ok_or("silicon.yaml must be a mapping")?;
    let key = |s: &str| serde_yaml::Value::String(s.to_owned());
    if !map[&key("access")].is_mapping() {
        return Err("access must be a mapping".into());
    }
    if !map[&key("flow")].is_sequence() {
        return Err("flow must be a sequence".into());
    }
    let silicon = map[&key("silicon")].as_mapping().unwrap();
    for field in ["id", "token", "timezone", "SILICON_HOME"] {
        if let Some(value) = silicon.get(key(field))
            && !value.is_string()
        {
            return Err(format!("silicon.{field} must be a string"));
        }
    }
    for (name, isi) in map[&key("isi")].as_mapping().unwrap() {
        let isi = isi
            .as_mapping()
            .ok_or("every isi entry must be a mapping")?;
        if let Some(dna) = isi.get(key("dna")) {
            let dna = dna.as_mapping().ok_or("isi DNA must be a mapping")?;
            if let Some(assemble) = dna.get(key("assemble")) {
                let assemble = assemble
                    .as_sequence()
                    .ok_or("DNA assemble must be a list")?;
                for item in assemble {
                    let reference = item
                        .as_str()
                        .ok_or("DNA assemble entries must be strings")?;
                    // Runtime expressions stay deferred, including commands, CEL, and fallback candidates.
                    if reference.starts_with('!')
                        || reference.contains('{')
                        || reference.contains("!>>")
                    {
                        continue;
                    }
                    let relative = safe_relative(reference)?;
                    let removed = removed.iter().any(|p| relative.starts_with(p));
                    if !output.join(relative).is_file()
                        && (removed || !project.join(relative).is_file())
                    {
                        return Err(format!(
                            "DNA for {} references missing output `{reference}`",
                            name.as_str().unwrap_or("isi")
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture(variables: &str, build: &str) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("starter.yaml"),format!("schema: 1\nvariables: {}\nfiles:\n  - from: silicon.yaml.tmpl\n    to: silicon.yaml\n{build}", if variables == "{}" { "{}".to_owned() } else { format!("\n{variables}") })).unwrap();
        fs::write(root.path().join("silicon.yaml.tmpl"), crate::SEED_YAML).unwrap();
        root
    }
    fn run(source: &Path, explicit: Value, saved: Value) -> Result<BuildResult> {
        let output = tempfile::tempdir().unwrap();
        build(
            source,
            &source.join("project"),
            output.path(),
            explicit.as_object().unwrap(),
            saved.as_object().unwrap(),
            false,
        )
    }

    #[test]
    fn sample_builds_both_branches_and_preserves_runtime_expressions() {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../starter_template/.starterbase");
        // The walkthrough is a repository fixture, not part of the published library crate.
        if !source.exists() {
            return;
        }
        assert!(compile(&source).unwrap().auto_update);
        for enabled in [false, true] {
            let output = tempfile::tempdir().unwrap();
            let answers = json!({"silicon_id":"a\"b\nc:tos","silicon_org_id":"lab","silicon_token":"secret","waveform":enabled,"timezone":"UTC","purpose":"{{ var.silicon_token }}"});
            let result = build(
                &source,
                &source.join("project"),
                output.path(),
                answers.as_object().unwrap(),
                &Map::new(),
                false,
            )
            .unwrap();
            let silicon = fs::read_to_string(output.path().join("silicon.yaml")).unwrap();
            assert!(silicon.contains("SILICON_HOME: ! pwd"));
            assert!(silicon.contains("{make_readable(request.tings)}"));
            let yaml = parse_yaml(&silicon).unwrap();
            assert_eq!(yaml["silicon"]["id"].as_str(), Some("a\"b\nc:tos"));
            assert_eq!(yaml["silicon"]["org_id"].as_str(), Some("lab"));
            assert_eq!(silicon.contains("waveform"), enabled);
            assert_eq!(
                result.answers.contains_key("waveform_tts_provider"),
                enabled
            );
            assert!(
                fs::read_to_string(output.path().join("prompts/silicon.md"))
                    .unwrap()
                    .contains("{{ var.silicon_token }}")
            );
            assert!(output.path().join("workspace").is_dir());
            assert!(output.path().join("memories").is_dir());
            assert!(!output.path().join("optional").exists());
        }
    }

    #[test]
    fn declaration_order_types_and_dependencies_are_checked_without_execution() {
        let root = fixture(
            "  z: {type: boolean, default: false}\n  a:\n    type: string\n    when: '{var.z}'\n    default: ! touch SHOULD_NOT_EXIST !>> 'fallback'",
            "",
        );
        let recipe = compile(root.path()).unwrap();
        assert_eq!(recipe.variables.keys().collect::<Vec<_>>(), vec!["z", "a"]);
        assert!(!root.path().join("SHOULD_NOT_EXIST").exists());
        run(root.path(), json!({}), json!({"a":"retained"})).unwrap();
        assert!(!root.path().join("SHOULD_NOT_EXIST").exists());
        for declaration in [
            "a: {type: string}",
            "a: {type: number, default: 1.5}",
            "a: {type: float, default: .nan}",
            "a: {type: integer, default: 1}",
            "a: {type: string, default: true}",
            "a: {type: string, default: x, required: true}",
            "a: {type: string, default: '{var.later} !>> \"ok\"'}\n  later: {type: string, default: later}",
            "a: {type: string, default: '{var.a} !>> \"ok\"'}",
            "a: {type: string, default: '{var[\"later\"]} !>> \"ok\"'}",
            "a: {type: string, default: '{1 +} !>> \"ok\"'}",
            "a: {type: string, default: '! echo hi'}",
            "a: {type: select, choices: [a, a], default: a}",
            "a: {type: multiselect, choices: [a], default: [a, a]}",
            "a: {type: string, default: x}\n  A: {type: string, default: y}",
        ] {
            assert!(
                parse_recipe(&format!("schema: 1\nvariables:\n  {declaration}\n")).is_err(),
                "accepted {declaration}"
            );
        }
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../starter_template/variables.yaml");
        if !path.exists() {
            return;
        }
        let catalog = parse_yaml(&fs::read_to_string(path).unwrap()).unwrap();
        let mut map = catalog.as_mapping().unwrap().clone();
        map.insert(
            serde_yaml::Value::String("schema".into()),
            serde_yaml::Value::Number(1.into()),
        );
        parse_recipe(&serde_yaml::to_string(&map).unwrap()).unwrap();
    }

    #[test]
    fn explicit_saved_defaults_inactive_answers_and_environment() {
        let root = fixture(
            r#"  enabled: {type: boolean, default: false}
  inactive:
    type: string
    when: '{var.enabled}'
    default: ! touch INACTIVE_RAN !>> "fallback"
  value:
    type: string
    default: ! touch DEFAULT_RAN; printf default !>> "fallback"
  count: {type: number, default: -1}
  rate: {type: float, default: 1.5}
  data: {type: object, default: {nested: [true, 2]}}
  items: {type: list, default: [one, 2]}
  choices: {type: multiselect, choices: [a, b], default: [b]}
  derived:
    type: string
    default: '{var.value + "!"} !>> "fallback"'
  math:
    type: number
    default: '{[1, 2, 3].map(x, x * 2)[1] + var.count} !>> 0'
"#,
            r#"build:
  - ! test "$STARTER_INTERACTIVE" = false && test -z "$STARTER_VAR_INACTIVE" && test "$STARTER_VAR_ENABLED" = false && test "$STARTER_VAR_COUNT" = -1 && test "$STARTER_VAR_DATA" = '\{"nested":[true,2]\}'
"#,
        );
        let result = run(
            root.path(),
            json!({"value":"explicit"}),
            json!({"value":"saved","inactive":"retain","unused":"retain"}),
        )
        .unwrap();
        assert_eq!(result.answers["value"], "explicit");
        assert_eq!(result.answers["inactive"], "retain");
        assert_eq!(result.answers["unused"], "retain");
        assert_eq!(result.answers["derived"], "explicit!");
        assert_eq!(result.answers["math"], 3);
        assert!(!root.path().join("DEFAULT_RAN").exists());
        assert!(!root.path().join("INACTIVE_RAN").exists());
        let result = run(root.path(), json!({}), json!({"value":"","enabled":false})).unwrap();
        assert_eq!(result.answers["derived"], "!");
        assert!(!root.path().join("DEFAULT_RAN").exists());
        assert!(run(root.path(), json!({"count":1.5}), json!({})).is_err());
        assert!(run(root.path(), json!({"unknown":"typo"}), json!({})).is_err());
    }

    #[test]
    fn fallback_is_only_for_evaluation_failure_and_choices_revalidate_saved_answers() {
        let root = fixture(
            r#"  empty:
    type: string
    default: ! printf '' !>> "fallback"
  no:
    type: boolean
    default: '{false} !>> true'
  count:
    type: number
    default: ! false !>> -3
  provider:
    type: select
    choices: ! printf '["google"]' !>> ["google"]
    default: google
"#,
            "",
        );
        let result = run(root.path(), json!({}), json!({})).unwrap();
        assert_eq!(result.answers["empty"], "");
        assert_eq!(result.answers["no"], false);
        assert_eq!(result.answers["count"], -3);
        assert!(
            run(root.path(), json!({}), json!({"provider":"gone"}))
                .unwrap_err()
                .contains("replacement")
        );
        let bad = fixture(
            "  count:\n    type: number\n    default: ! printf 1.5 !>> 2",
            "",
        );
        assert!(run(bad.path(), json!({}), json!({})).is_err());
        let bad = fixture(
            "  a:\n    type: select\n    default: x\n    choices: ! printf nope !>> [x]",
            "",
        );
        assert!(run(bad.path(), json!({}), json!({})).is_err());
        let bad = fixture("  a: {type: string, when: '{\"false\"}', default: x}", "");
        assert!(
            run(bad.path(), json!({}), json!({}))
                .unwrap_err()
                .contains("boolean")
        );
    }

    #[test]
    fn jinja_paths_yaml_and_hooks_fail_before_application() {
        let root = fixture("  value: {type: string, default: hello}", "");
        fs::write(
            root.path().join("silicon.yaml.tmpl"),
            format!("{}\n{{% include \"part.md\" %}}", crate::SEED_YAML),
        )
        .unwrap();
        fs::write(root.path().join("part.md"), "# {{ var.missing }}\n").unwrap();
        assert!(run(root.path(), json!({}), json!({})).is_err());
        fs::write(
            root.path().join("part.md"),
            "# {% raw %}{{ example }}{% endraw %}\n",
        )
        .unwrap();
        run(root.path(), json!({}), json!({})).unwrap();
        // Non-template sources remain verbatim, even when they contain incomplete Jinja text.
        fs::write(root.path().join("literal.txt"), "{{ incomplete").unwrap();
        let mut recipe = compile(root.path()).unwrap();
        recipe.files.push(FileSpec {
            from: "literal.txt".into(),
            to: "literal.txt".into(),
        });
        fs::write(
            root.path().join("starter.yaml"),
            serde_yaml::to_string(&recipe).unwrap(),
        )
        .unwrap();
        run(root.path(), json!({}), json!({})).unwrap();
        fs::write(
            root.path().join("silicon.yaml.tmpl"),
            "{% include '../secret' %}",
        )
        .unwrap();
        assert!(run(root.path(), json!({}), json!({})).is_err());
        let bad = fixture(
            "{}",
            "build:\n  - ! printf 'broken: [' > \"$STARTER_OUTPUT/silicon.yaml\"",
        );
        assert!(run(bad.path(), json!({}), json!({})).is_err());
        let bad = fixture("{}", "build:\n  - ! false");
        assert!(run(bad.path(), json!({}), json!({})).is_err());
        for path in [
            "../outside",
            "/absolute",
            ".git/config",
            ".GIT/config",
            ".starterbase/state",
            ".STARTERBASE/state",
            ".state/x",
        ] {
            assert!(
                parse_recipe(&format!(
                    "schema: 1\nfiles: [{{from: input, to: '{path}'}}]"
                ))
                .is_err()
            );
        }
        assert!(
            parse_recipe("schema: 1\nfiles: [{from: a, to: path}, {from: b, to: path/child}]")
                .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_sources_and_output_are_rejected() {
        let root = fixture("{}", "");
        std::os::unix::fs::symlink("/etc/passwd", root.path().join("escape")).unwrap();
        assert!(compile(root.path()).is_err());
        fs::remove_file(root.path().join("escape")).unwrap();
        let root = fixture(
            "{}",
            "build:\n  - ! ln -s /etc/passwd \"$STARTER_OUTPUT/escape\"",
        );
        assert!(run(root.path(), json!({}), json!({})).is_err());
    }

    #[test]
    fn settings_round_trip_bare_commands_and_preserve_literals() {
        let root = fixture(
            "  a:\n    type: string\n    default: ! printf works !>> 'fallback'",
            "",
        );
        set_auto_update(root.path(), true).unwrap();
        assert!(auto_update(root.path()).unwrap());
        assert_eq!(
            run(root.path(), json!({}), json!({})).unwrap().answers["a"],
            "works"
        );
        let yaml = parse_yaml(
            "command: ! echo hello # comment\nquoted: \"! echo hi\"\nblock: |\n  ! echo literal\n",
        )
        .unwrap();
        assert_eq!(yaml["command"].as_str(), Some("! echo hello"));
        assert_eq!(yaml["block"].as_str(), Some("! echo literal\n"));
    }
}
