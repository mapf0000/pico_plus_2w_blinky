use dsl_core::{self as core, DslError, DslErrorAt, LayoutParseError};
use std::str::FromStr;

struct BuiltinProvider;

impl core::ScriptProvider for BuiltinProvider {
    fn get<'a>(&self, id: &'a str) -> Option<&'a str> {
        crate::scripts::lookup(id)
    }
}

pub struct CompileError {
    pub message: String,
}

pub fn compile(entry_dsl: &str, default_layout: &str) -> Result<Vec<u8>, CompileError> {
    let provider = BuiltinProvider;
    let program = core::compile_and_link(entry_dsl, &provider).map_err(from_compile_err)?;
    let layout = core::LayoutId::from_str(default_layout).map_err(|err| {
        let message = match err {
            LayoutParseError::Unknown => {
                format!("Unknown default layout: {}", default_layout)
            }
            LayoutParseError::NotEnabled => format!(
                "Default layout not enabled at build time: {}",
                default_layout
            ),
        };
        CompileError { message }
    })?;
    let flat = core::lower_to_flat_with_layout(&program, layout).map_err(from_compile_err)?;
    Ok(core::bytecode::encode(&flat))
}

fn from_compile_err(err: DslErrorAt) -> CompileError {
    let base = match err.kind {
        DslError::TooManyLines => "Program exceeds maximum length",
        DslError::UnknownCommand => "Unknown command",
        DslError::InvalidLine => "Invalid syntax",
        DslError::ParseKey => "Unrecognized key name",
        DslError::ParseMod => "Unrecognized modifier",
        DslError::ParseDelay => "Invalid delay value",
        DslError::TextEmpty => "text() requires a non-empty string",
        DslError::UnknownLayout => "Unknown layout id",
        DslError::LayoutNotEnabled => "Layout not enabled at build time",
        DslError::UnknownScript => "call refers to unknown script",
        DslError::RecursionTooDeep => "Recursive call detected",
    };
    let message = if err.line > 0 {
        format!("{base} (line {})", err.line)
    } else {
        base.to_string()
    };
    CompileError { message }
}
