use dsl_core::{self as core, CompileError as CoreCompileError};

struct BuiltinProvider;

impl core::ScriptProvider for BuiltinProvider {
    fn get<'a>(&self, id: &'a str) -> Option<&'a str> {
        crate::scripts::lookup(id)
    }
}

pub struct CompileError {
    pub message: String,
}

pub fn compile(entry_dsl: &str) -> Result<Vec<u8>, CompileError> {
    let provider = BuiltinProvider;
    let program = core::compile_and_link_with_required_layout(entry_dsl, &provider)
        .map_err(from_compile_err)?;
    let flat = core::lower_to_flat(&program).map_err(from_compile_err)?;
    let bytes = core::bytecode::encode(&flat).map_err(|_| CompileError {
        message: "Program exceeds maximum length".into(),
    })?;
    Ok(bytes)
}

pub fn entry_layout(dsl: &str) -> Option<&str> {
    dsl.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .and_then(parse_layout_declaration)
}

pub fn set_entry_layout(dsl: &str, layout: &str) -> String {
    let declaration = format!("layout(\"{layout}\")");
    let mut offset = 0usize;

    for segment in dsl.split_inclusive('\n') {
        let line = segment.strip_suffix('\n').unwrap_or(segment);
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            offset += segment.len();
            continue;
        }

        if parse_layout_declaration(trimmed).is_some() {
            let indent_len = line.len() - line.trim_start().len();
            let mut updated = dsl.to_string();
            updated.replace_range(offset + indent_len..offset + line.len(), &declaration);
            return updated;
        }

        let mut updated = String::with_capacity(dsl.len() + declaration.len() + 1);
        updated.push_str(&dsl[..offset]);
        updated.push_str(&declaration);
        updated.push('\n');
        updated.push_str(&dsl[offset..]);
        return updated;
    }

    let mut updated = dsl.to_string();
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(&declaration);
    updated.push('\n');
    updated
}

fn parse_layout_declaration(line: &str) -> Option<&str> {
    let args = line.strip_prefix("layout(")?.strip_suffix(')')?.trim();
    args.strip_prefix('"')?.strip_suffix('"')
}

fn from_compile_err(err: CoreCompileError) -> CompileError {
    let base = if err.message.is_empty() {
        err.code.to_string()
    } else {
        err.message
    };
    let message = if err.span.line > 0 {
        format!("{base} (line {})", err.span.line)
    } else {
        base
    };
    CompileError { message }
}

#[cfg(test)]
mod tests {
    use super::{compile, entry_layout, set_entry_layout};

    #[test]
    fn reads_leading_layout_after_comments() {
        let dsl = "# comment\n\nlayout(\"mac_de-DE\")\ntext(\"Hi\")\n";
        assert_eq!(entry_layout(dsl), Some("mac_de-DE"));
    }

    #[test]
    fn selector_replaces_leading_layout() {
        let dsl = "layout(\"win_en-US\")\ntext(\"Hi\")\n";
        let updated = set_entry_layout(dsl, "mac_de-DE");
        assert_eq!(updated, "layout(\"mac_de-DE\")\ntext(\"Hi\")\n");
    }

    #[test]
    fn selector_inserts_layout_after_leading_comments() {
        let dsl = "# comment\ntext(\"Hi\")\n";
        let updated = set_entry_layout(dsl, "win_en-US");
        assert_eq!(updated, "# comment\nlayout(\"win_en-US\")\ntext(\"Hi\")\n");
    }

    #[test]
    fn all_builtins_compile_with_declared_layouts() {
        for script in crate::scripts::all() {
            compile(script.dsl).unwrap_or_else(|err| {
                panic!("compile builtin {} failed: {}", script.id, err.message)
            });
        }
    }
}
