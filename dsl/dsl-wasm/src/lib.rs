use wasm_bindgen::prelude::*;
use dsl_core::{ self as core, DslError, DslErrorAt };

#[wasm_bindgen]
pub struct WasmDiagnostic {
    pub line: u16,
    pub code: String,
    pub message: String,
}

fn diag(err: DslErrorAt) -> JsValue {
    let code = match err.kind {
        DslError::TooManyLines => "TooManyLines",
        DslError::UnknownCommand => "UnknownCommand",
        DslError::InvalidLine => "InvalidLine",
        DslError::ParseKey => "ParseKey",
        DslError::ParseMod => "ParseMod",
        DslError::ParseDelay => "ParseDelay",
        DslError::TextEmpty => "TextEmpty",
        DslError::UnknownScript => "UnknownScript",
        DslError::RecursionTooDeep => "RecursionTooDeep",
    }.to_string();
    let wd = WasmDiagnostic { line: err.line, code: code.clone(), message: format!("{} at line {}", code, err.line) };
    JsValue::from_serde(&wd).unwrap_or_else(|_| JsValue::from_str(&format!("{:?}", wd)))
}

/// Compile a DSL entry script and a set of named scripts (by JSON id→text)
/// into bytecode suitable for the Pi executor.
/// - `scripts_json`: a JSON object like {"hello":"text ...", "foo":"..."}
#[wasm_bindgen]
pub fn compile_to_bytecode(entry_dsl: &str, scripts_json: &str) -> Result<Box<[u8]>, JsValue> {
    use std::collections::HashMap;
    let scripts: HashMap<String, String> =
        serde_json::from_str(scripts_json).map_err(|e| JsValue::from_str(&e.to_string()))?;

    let provider = |id: &str| scripts.get(id).map(|s| s.as_str());

    // 1) compile + link (inline calls)
    let owned = core::compile_and_link(entry_dsl, &provider).map_err(diag)?;

    // 2) lower to US flat ops
    let flat = core::lower_to_flat_us(&owned).map_err(diag)?;

    // 3) encode to bytecode
    let bytes = core::bytecode::encode(&flat);
    Ok(bytes.into_boxed_slice())
}

/// Quick syntax check of a single DSL text (no linking, no lowering).
#[wasm_bindgen]
pub fn lint_dsl(dsl: &str) -> Result<(), JsValue> {
    let accept_all = |_| true;
    core::compile_dsl_with_diag(dsl, accept_all).map(|_| ()).map_err(diag)
}
