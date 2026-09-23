//! Preserve the bare Bash marker using the same scalar convention as Stemcell.
/// Only rewrite scalar starts. Literal/folded blocks and quoted YAML remain
/// untouched; YAML still owns indentation, escaping, comments and collections.
pub(super) fn prepare_scalars(source: &str) -> Result<String, String> {
    let mut output = Vec::new();
    let mut block_indent = None;
    let mut inline = (0usize, None, true);
    for (line_number, line) in source.lines().enumerate() {
        let indent = line.len() - line.trim_start_matches(' ').len();
        if inline.0 > 0 || inline.1.is_some() {
            check_inline_scalar(line, line_number + 1, &mut inline)?;
            output.push(line.to_owned());
            continue;
        }
        if block_indent.is_some_and(|base| line.trim().is_empty() || indent > base) {
            output.push(line.to_owned());
            continue;
        }
        block_indent = None;
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            output.push(line.to_owned());
            continue;
        }
        let content = &line[indent..];
        let seq_prefix = if content.starts_with("- ") { 2 } else { 0 };
        let after_seq = &content[seq_prefix..];
        let scalar_offset = mapping_colon(after_seq)
            .map(|i| indent + seq_prefix + i + 1)
            .unwrap_or(indent + seq_prefix);
        let whitespace = line[scalar_offset..].len() - line[scalar_offset..].trim_start().len();
        let start = scalar_offset + whitespace;
        let scalar = &line[start..];
        if scalar.starts_with(['|', '>']) {
            block_indent = Some(indent);
            output.push(line.to_owned());
            continue;
        }
        let quoted = scalar.starts_with(['\'', '"', '[', '{']);
        if quoted {
            inline.2 = true;
            check_inline_scalar(scalar, line_number + 1, &mut inline)?;
            output.push(line.to_owned());
            continue;
        }
        if scalar.starts_with('!') || (!quoted && scalar.contains(": ")) {
            let value = strip_comment(scalar).trim_end();
            let value = if let Some(command) = value.strip_prefix('!') {
                format!("! {}", command.trim_start())
            } else {
                value.to_owned()
            };
            output.push(format!(
                "{}{}",
                &line[..start],
                serde_json::to_string(&value).map_err(|e| e.to_string())?
            ));
        } else {
            output.push(line.to_owned());
        }
    }
    Ok(output.join("\n") + "\n")
}

// Track only quoted scalars and flow collections, leaving their syntax to YAML.
// YAML otherwise discards the anonymous `!` tag before our DSL sees the command.
fn check_inline_scalar(
    source: &str,
    line: usize,
    state: &mut (usize, Option<u8>, bool),
) -> Result<(), String> {
    let bytes = source.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let ch = bytes[i];
        if let Some(quote) = state.1 {
            if ch == b'\\' && quote == b'"' {
                i += 2;
                continue;
            }
            if ch == quote {
                if quote == b'\'' && bytes.get(i + 1) == Some(&quote) {
                    i += 2;
                    continue;
                }
                state.1 = None;
                state.2 = false;
                if state.0 == 0 {
                    break;
                }
            }
        } else {
            match ch {
                b'#' if i == 0 || bytes[i - 1].is_ascii_whitespace() => break,
                b'\'' | b'"' if state.2 => state.1 = Some(ch),
                b'[' | b'{' => {
                    state.0 += 1;
                    state.2 = true;
                }
                b']' | b'}' => {
                    state.0 = state.0.saturating_sub(1);
                    state.2 = false;
                }
                b',' => state.2 = true,
                b':' if bytes
                    .get(i + 1)
                    .is_none_or(|next| next.is_ascii_whitespace()) =>
                {
                    state.2 = true
                }
                b'!' if state.0 > 0
                    && state.2
                    && bytes
                        .get(i + 1)
                        .is_none_or(|next| next.is_ascii_whitespace()) =>
                {
                    return Err(format!(
                        "bare ! command in inline YAML collection on line {line}; quote the complete expression"
                    ));
                }
                ch if !ch.is_ascii_whitespace() => state.2 = false,
                _ => {}
            }
        }
        i += 1;
    }
    Ok(())
}

fn mapping_colon(source: &str) -> Option<usize> {
    let mut quote = None;
    let mut escaped = false;
    for (i, ch) in source.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote == Some('"') {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            continue;
        }
        if ch == ':'
            && source[i + 1..]
                .chars()
                .next()
                .is_none_or(char::is_whitespace)
        {
            return Some(i);
        }
        if ch == '[' || ch == '{' || ch == '!' {
            return None;
        }
    }
    None
}

fn strip_comment(source: &str) -> &str {
    let mut quote = None;
    let mut escaped = false;
    for (i, ch) in source.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote == Some('"') {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            continue;
        }
        if ch == '#' && (i == 0 || source[..i].ends_with(char::is_whitespace)) {
            return &source[..i];
        }
    }
    source
}
