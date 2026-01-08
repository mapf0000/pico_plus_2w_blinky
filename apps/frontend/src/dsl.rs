use dsl_core::{self as core, CompileError as CoreCompileError, LayoutParseError};
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
    let bytes = core::bytecode::encode(&flat).map_err(|_| CompileError {
        message: "Program exceeds maximum length".into(),
    })?;
    Ok(bytes)
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
