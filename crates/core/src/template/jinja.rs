//! Use MiniJinja's own parser to inspect constant includes without rendering recipes.
use super::{Recipe, Result, source_file};
use minijinja::{
    Environment,
    machinery::{
        ast::{Expr, Stmt},
        parse,
    },
};
use std::{collections::HashSet, fs, path::Path};

pub(super) fn validate(
    source: &Path,
    env: &Environment<'_>,
    name: &str,
    recipe: &Recipe,
    checked: &mut HashSet<String>,
) -> Result<()> {
    if !checked.insert(name.to_owned()) {
        return Ok(());
    }
    let text = fs::read_to_string(source_file(source, name)?).map_err(|e| e.to_string())?;
    let template = env
        .get_template(name)
        .map_err(|e| format!("invalid template {name}: {e}"))?;
    for reference in template.undeclared_variables(true) {
        if let Some(variable) = reference
            .strip_prefix("var.")
            .and_then(|s| s.split('.').next())
            && !recipe.variables.contains_key(variable)
        {
            return Err(format!(
                "template {name} references undeclared variable var.{variable}"
            ));
        }
    }
    let ast =
        parse(&text, name, Default::default(), Default::default()).map_err(|e| e.to_string())?;
    let mut includes = Vec::new();
    references(&ast, &mut includes);
    for (name, optional) in includes {
        if optional && !source.join(super::safe_relative(&name)?).exists() {
            continue;
        }
        validate(source, env, &name, recipe, checked)?;
    }
    Ok(())
}

fn name(expr: &Expr<'_>, optional: bool, found: &mut Vec<(String, bool)>) {
    if let Some(value) = expr.as_const()
        && let Some(name) = value.as_str()
    {
        found.push((name.to_owned(), optional));
    }
}
fn children(stmts: &[Stmt<'_>], found: &mut Vec<(String, bool)>) {
    for stmt in stmts {
        references(stmt, found);
    }
}
fn references(stmt: &Stmt<'_>, found: &mut Vec<(String, bool)>) {
    match stmt {
        Stmt::Template(v) => children(&v.children, found),
        Stmt::ForLoop(v) => {
            children(&v.body, found);
            children(&v.else_body, found);
        }
        Stmt::IfCond(v) => {
            children(&v.true_body, found);
            children(&v.false_body, found);
        }
        Stmt::WithBlock(v) => children(&v.body, found),
        Stmt::SetBlock(v) => children(&v.body, found),
        Stmt::AutoEscape(v) => children(&v.body, found),
        Stmt::FilterBlock(v) => children(&v.body, found),
        Stmt::Block(v) => children(&v.body, found),
        Stmt::Macro(v) => children(&v.body, found),
        Stmt::CallBlock(v) => children(&v.macro_decl.body, found),
        Stmt::Include(v) => name(&v.name, v.ignore_missing, found),
        Stmt::Extends(v) => name(&v.name, false, found),
        Stmt::Import(v) => name(&v.expr, false, found),
        Stmt::FromImport(v) => name(&v.expr, false, found),
        _ => {}
    }
}
