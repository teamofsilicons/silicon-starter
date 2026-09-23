//! Stemcell's CEL → explicit Bash → fallback notation, without runtime side effects.
use cel_interpreter::{Context, Program, Value as Cel};
use cel_parser::{
    Parser,
    ast::{EntryExpr, Expr, IdedExpr},
    reference::Val,
};
use serde_json::{Map, Value};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    process::{Command, Stdio},
};

type Result<T> = std::result::Result<T, String>;

#[derive(Debug)]
enum Part {
    Text(String),
    Cel(String),
}

pub(super) fn text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}

fn cel_value(value: &Value) -> Cel {
    match value {
        Value::Null => Cel::Null,
        Value::Bool(v) => Cel::Bool(*v),
        Value::String(v) => v.clone().into(),
        Value::Number(v) => {
            if let Some(n) = v.as_i64() {
                Cel::Int(n)
            } else if let Some(n) = v.as_u64() {
                Cel::UInt(n)
            } else {
                Cel::Float(v.as_f64().unwrap())
            }
        }
        Value::Array(v) => v.iter().map(cel_value).collect::<Vec<_>>().into(),
        Value::Object(v) => v
            .iter()
            .map(|(k, v)| (k.clone(), cel_value(v)))
            .collect::<HashMap<_, _>>()
            .into(),
    }
}

fn program(source: &str) -> Result<Program> {
    std::panic::catch_unwind(|| Program::compile(source))
        .map_err(|_| "CEL parser rejected malformed input".to_string())?
        .map_err(|e| format!("invalid CEL: {e}"))
}

fn cel(source: &str, active: &Map<String, Value>) -> Result<Value> {
    let program = program(source)?;
    let mut context = Context::default();
    context.add_variable_from_value("var", cel_value(&Value::Object(active.clone())));
    let value =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| program.execute(&context)))
            .map_err(|_| "CEL evaluation failed".to_string())?
            .map_err(|e| format!("CEL evaluation failed: {e}"))?;
    value.json().map_err(|e| e.to_string())
}

/// Split separators outside shell quotes and CEL, including nested map literals.
pub(super) fn candidates(source: &str) -> Result<Vec<&str>> {
    let bytes = source.as_bytes();
    let (mut start, mut i, mut depth) = (0, 0, 0usize);
    let mut quote = None;
    let mut result = Vec::new();
    while i < bytes.len() {
        match (bytes[i], quote) {
            (b'\\', _) => {
                i += 2;
                continue;
            }
            (c, Some(q)) if c == q => quote = None,
            (_, Some(_)) => {}
            (b'\'' | b'"', None) => quote = Some(bytes[i]),
            (b'{' | b'[', None) => depth += 1,
            (b'}' | b']', None) => depth = depth.saturating_sub(1),
            (b'!', None) if depth == 0 && source[i..].starts_with("!>>") => {
                result.push(source[start..i].trim());
                start = i + 3;
                i += 3;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    result.push(if result.is_empty() {
        source
    } else {
        source[start..].trim()
    });
    if result.len() > 1 && result.iter().any(|c| c.is_empty()) {
        return Err("empty expression fallback".into());
    }
    Ok(result)
}

fn parts(source: &str) -> Result<Vec<Part>> {
    let bytes = source.as_bytes();
    let mut result = Vec::new();
    let mut literal = String::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && bytes
                .get(i + 1)
                .is_some_and(|c| matches!(c, b'{' | b'}' | b'\\'))
        {
            literal.push(bytes[i + 1] as char);
            i += 2;
        } else if bytes[i] == b'{' {
            if !literal.is_empty() {
                result.push(Part::Text(std::mem::take(&mut literal)));
            }
            let start = i + 1;
            let mut depth = 1;
            let mut quote = None;
            i += 1;
            while i < bytes.len() && depth > 0 {
                match (bytes[i], quote) {
                    (b'\\', Some(_)) => {
                        i += 2;
                        continue;
                    }
                    (c, Some(q)) if c == q => quote = None,
                    (_, Some(_)) => {}
                    (b'\'' | b'"', None) => quote = Some(bytes[i]),
                    (b'{', None) => depth += 1,
                    (b'}', None) => depth -= 1,
                    _ => {}
                }
                i += 1;
            }
            if depth != 0 {
                return Err("unclosed CEL interpolation".into());
            }
            result.push(Part::Cel(source[start..i - 1].to_owned()));
        } else if bytes[i] == b'}' {
            return Err("unescaped closing CEL brace (use \\} for text)".into());
        } else {
            let c = source[i..].chars().next().unwrap();
            literal.push(c);
            i += c.len_utf8();
        }
    }
    if !literal.is_empty() {
        result.push(Part::Text(literal));
    }
    Ok(result)
}

pub(super) fn dynamic(value: &Value) -> bool {
    value.as_str().is_some_and(|s| {
        s.trim_start().starts_with('!')
            || s.contains("!>>")
            || parts(s)
                .map(|p| p.iter().any(|p| matches!(p, Part::Cel(_))))
                .unwrap_or(s.contains('{') || s.contains('}'))
    })
}

/// Fallbacks are native JSON/YAML literal values, never another expression.
pub(super) fn literal(source: &str) -> Result<Value> {
    if source.trim_start().starts_with('{') && serde_json::from_str::<Value>(source).is_err() {
        return Err("object fallbacks must be literal JSON objects, not CEL expressions".into());
    }
    let value: Value =
        serde_yaml::from_str(source).map_err(|e| format!("invalid literal fallback: {e}"))?;
    if value.is_null() || source.trim_start().starts_with('!') {
        return Err("fallback must be a non-null literal".into());
    }
    Ok(value)
}

fn literal_candidate(source: &str) -> Option<Value> {
    let source = source.trim();
    // A quoted scalar or JSON collection is data; its braces are not CEL.
    if source.starts_with(['\'', '"', '['])
        || serde_json::from_str::<Value>(source).is_ok_and(|v| v.is_object())
    {
        literal(source).ok()
    } else {
        None
    }
}

fn check_ref(name: &str, known: &HashSet<String>) -> Result<()> {
    if known.contains(name) {
        Ok(())
    } else {
        Err(format!("unknown or forward variable reference var.{name}"))
    }
}

fn check_ast(node: &IdedExpr, known: &HashSet<String>, locals: &HashSet<String>) -> Result<()> {
    match &node.expr {
        Expr::Ident(name) if !locals.contains(name) => {
            return Err(format!(
                "unknown CEL identifier `{name}`; access earlier answers as var.name"
            ));
        }
        Expr::Select(select) => {
            if matches!(&select.operand.expr, Expr::Ident(v) if v == "var") {
                check_ref(&select.field, known)?;
            } else {
                check_ast(&select.operand, known, locals)?;
            }
        }
        Expr::Call(call) => {
            if call.func_name == "_[_]"
                && call
                    .args
                    .first()
                    .is_some_and(|v| matches!(&v.expr,Expr::Ident(n) if n == "var"))
            {
                match call.args.get(1).map(|v| &v.expr) {
                    Some(Expr::Literal(Val::String(name))) => check_ref(name, known)?,
                    _ => return Err("var indices must be literal variable names".into()),
                }
            } else {
                if let Some(target) = &call.target {
                    check_ast(target, known, locals)?;
                }
                for arg in &call.args {
                    check_ast(arg, known, locals)?;
                }
            }
        }
        Expr::Comprehension(comp) => {
            check_ast(&comp.iter_range, known, locals)?;
            check_ast(&comp.accu_init, known, locals)?;
            let mut bound = locals.clone();
            bound.insert(comp.iter_var.clone());
            bound.insert(comp.accu_var.clone());
            if let Some(name) = &comp.iter_var2 {
                bound.insert(name.clone());
            }
            for child in [&comp.loop_cond, &comp.loop_step, &comp.result] {
                check_ast(child, known, &bound)?;
            }
        }
        Expr::List(list) => {
            for child in &list.elements {
                check_ast(child, known, locals)?;
            }
        }
        Expr::Map(map) => {
            for entry in &map.entries {
                check_entry(&entry.expr, known, locals)?;
            }
        }
        Expr::Struct(map) => {
            for entry in &map.entries {
                check_entry(&entry.expr, known, locals)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn check_entry(entry: &EntryExpr, known: &HashSet<String>, locals: &HashSet<String>) -> Result<()> {
    match entry {
        EntryExpr::StructField(field) => check_ast(&field.value, known, locals),
        EntryExpr::MapEntry(entry) => {
            check_ast(&entry.key, known, locals)?;
            check_ast(&entry.value, known, locals)
        }
    }
}

pub(super) fn validate(source: &str, known: &HashSet<String>) -> Result<()> {
    for candidate in candidates(source)? {
        if literal_candidate(candidate).is_some() {
            continue;
        }
        let command = candidate.trim_start().strip_prefix('!');
        let body = command.unwrap_or(candidate).trim_start();
        let body = if command.is_some() {
            literal_candidate(body)
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_else(|| body.to_owned())
        } else {
            body.to_owned()
        };
        let mut shell = String::new();
        for part in parts(&body)? {
            match part {
                Part::Text(s) => shell.push_str(&s),
                Part::Cel(s) => {
                    program(&s)?;
                    let ast = std::panic::catch_unwind(|| Parser::default().parse(&s))
                        .map_err(|_| "invalid CEL".to_string())?
                        .map_err(|e| e.to_string())?;
                    check_ast(&ast, known, &HashSet::new())?;
                    shell.push_str("CEL_VALUE");
                }
            }
        }
        if command.is_some() {
            if shell.trim().is_empty() {
                return Err("empty Bash command".into());
            }
            let result = Command::new("bash")
                .env_remove("BASH_ENV")
                .env_remove("ENV")
                .args(["-n", "-c", &shell])
                .stdin(Stdio::null())
                .output()
                .map_err(|e| format!("cannot validate Bash: {e}"))?;
            if !result.status.success() {
                return Err(format!(
                    "invalid Bash syntax: {}",
                    String::from_utf8_lossy(&result.stderr).trim()
                ));
            }
            for tail in shell.split("$STARTER_VAR_").skip(1) {
                let name: String = tail
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                if !known.iter().any(|k| k.to_ascii_uppercase() == name) {
                    return Err(format!(
                        "unknown or forward shell variable STARTER_VAR_{name}"
                    ));
                }
            }
        }
    }
    Ok(())
}

pub(super) struct Runtime<'a> {
    pub source: &'a Path,
    pub project: &'a Path,
    pub output: Option<&'a Path>,
    pub interactive: bool,
    pub active: &'a Map<String, Value>,
}
impl Runtime<'_> {
    fn candidate(&self, candidate: &str) -> Result<Value> {
        if let Some(value) = literal_candidate(candidate) {
            return Ok(value);
        }
        let command = candidate.trim_start().strip_prefix('!');
        let body = command.unwrap_or(candidate);
        let body = if command.is_some() {
            literal_candidate(body.trim())
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_else(|| body.trim_start().to_owned())
        } else {
            body.to_owned()
        };
        let parts = parts(&body)?;
        if command.is_none()
            && parts.len() == 1
            && let Part::Cel(source) = &parts[0]
        {
            return cel(source, self.active);
        }
        let mut expanded = String::new();
        for part in parts {
            expanded.push_str(&match part {
                Part::Text(s) => s,
                Part::Cel(s) => text(&cel(&s, self.active)?),
            });
        }
        if command.is_none() {
            return Ok(match expanded.trim() {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                _ => Value::String(expanded),
            });
        }
        let mut cmd = Command::new("bash");
        cmd.args(["-c", &expanded])
            .current_dir(self.source)
            .stdin(if self.interactive {
                Stdio::inherit()
            } else {
                Stdio::null()
            })
            .stderr(Stdio::inherit());
        // Never leak an inactive answer inherited from a parent starter process.
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("STARTER_VAR_") {
                cmd.env_remove(key);
            }
        }
        cmd.env("STARTER_SOURCE", self.source)
            .env("STARTER_PROJECT", self.project)
            .env(
                "STARTER_INTERACTIVE",
                if self.interactive { "true" } else { "false" },
            );
        cmd.env_remove("STARTER_OUTPUT");
        if let Some(output) = self.output {
            cmd.env("STARTER_OUTPUT", output);
        }
        for (key, value) in self.active {
            cmd.env(
                format!("STARTER_VAR_{}", key.to_ascii_uppercase()),
                text(value),
            );
        }
        if self.output.is_some() {
            let status = cmd
                .stdout(Stdio::inherit())
                .status()
                .map_err(|e| format!("cannot run Bash: {e}"))?;
            return if status.success() {
                Ok(Value::Null)
            } else {
                Err(format!("Bash exited with {status}"))
            };
        }
        let result = cmd.output().map_err(|e| format!("cannot run Bash: {e}"))?;
        if !result.status.success() {
            return Err(format!("Bash exited with {}", result.status));
        }
        Ok(Value::String(
            String::from_utf8(result.stdout)
                .map_err(|_| "Bash output is not UTF-8")?
                .trim_end_matches(['\r', '\n'])
                .to_owned(),
        ))
    }

    pub fn evaluate(&self, source: &str) -> Result<Value> {
        let mut error = String::new();
        for candidate in candidates(source)? {
            match self.candidate(candidate) {
                Ok(value) => return Ok(value),
                Err(e) => error = e,
            }
        }
        Err(format!("all expression candidates failed: {error}"))
    }
}
