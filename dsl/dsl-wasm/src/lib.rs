use dsl_core::{self as core, DslError, DslErrorAt, PreError};
use std::collections::{HashMap, HashSet, VecDeque};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
#[derive(serde::Serialize)]
pub struct WasmDiagnostic {
    pub severity: String,
    pub line: u16,
    pub col: u16,
    pub span_len: u16,
    pub code: String,
    pub message: String,
    pub suggestion: Option<String>,
    pub script: Option<String>,
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
    }
    .to_string();
    let wd = WasmDiagnostic {
        severity: "error".into(),
        line: err.line,
        col: 0,
        span_len: 0,
        code: code.clone(),
        message: format!("{} at line {}", code, err.line),
        suggestion: None,
        script: None,
    };
    JsValue::from_serde(&wd).unwrap_or_else(|_| JsValue::from_str(&format!("{:?}", wd)))
}

fn diag_pre(err: PreError) -> JsValue {
    let wd = WasmDiagnostic {
        severity: "error".into(),
        line: err.line,
        col: err.col,
        span_len: 0,
        code: err.code.into(),
        message: err.message,
        suggestion: None,
        script: None,
    };
    JsValue::from_serde(&wd).unwrap_or_else(|_| JsValue::from_str(&format!("{:?}", wd)))
}

/// Compile a DSL entry script and a set of named scripts (by JSON id→text)
/// into bytecode suitable for the Pi executor.
/// - `scripts_json`: a JSON object like {"hello":"text ...", "foo":"..."}
#[wasm_bindgen]
pub fn compile_to_bytecode(entry_dsl: &str, scripts_json: &str) -> Result<Box<[u8]>, JsValue> {
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
    let pre = core::preprocess(dsl, &core::PreprocessOptions::default()).map_err(diag_pre)?;
    let accept_all = |_| true;
    match core::compile_dsl_with_diag(&pre.text, accept_all) {
        Ok(_) => Ok(()),
        Err(e) => {
            let mapped_line = pre
                .sourcemap
                .get((e.line as usize).saturating_sub(1))
                .map(|loc| loc.line)
                .unwrap_or(e.line);
            Err(diag(DslErrorAt {
                kind: e.kind,
                line: mapped_line,
            }))
        }
    }
}

/// Lint entry + scripts, returning all diagnostics (errors and warnings) as JSON.
/// - Preprocesses entry and each script (file-local scopes) to collect warnings (repeat/let, basic lints).
/// - Attempts to compile+link to surface parse/link errors.
/// - Reports unused scripts.
#[wasm_bindgen]
pub fn lint_dsl_all(entry_dsl: &str, scripts_json: &str) -> Result<JsValue, JsValue> {
    let scripts: HashMap<String, String> =
        serde_json::from_str(scripts_json).map_err(|e| JsValue::from_str(&e.to_string()))?;

    let mut out: Vec<WasmDiagnostic> = Vec::new();

    // 1) Preprocess entry
    let mut entry_pre_text: Option<String> = None;
    let mut entry_pre_map: Option<Vec<core::OrigLoc>> = None;
    let mut pre_scripts: HashMap<String, String> = HashMap::new();
    let mut pre_maps: HashMap<String, Vec<core::OrigLoc>> = HashMap::new();

    match core::preprocess(entry_dsl, &core::PreprocessOptions::default()) {
        Ok(pre) => {
            entry_pre_text = Some(pre.text);
            entry_pre_map = Some(pre.sourcemap);
            // warnings
            for d in pre.diags {
                out.push(WasmDiagnostic {
                    severity: match d.severity {
                        core::Severity::Warning => "warning".into(),
                        core::Severity::Error => "error".into(),
                    },
                    line: d.line,
                    col: d.col,
                    span_len: d.span_len,
                    code: d.code.into(),
                    message: d.message,
                    suggestion: d.suggestion,
                    script: None,
                });
            }
        }
        Err(e) => {
            out.push(WasmDiagnostic {
                severity: "error".into(),
                line: e.line,
                col: e.col,
                span_len: 0,
                code: e.code.into(),
                message: e.message,
                suggestion: None,
                script: None,
            });
            // If preprocess fails, still return what we have
            return JsValue::from_serde(&out).map_err(|e| JsValue::from_str(&e.to_string()));
        }
    }

    // 2) Preprocess all scripts for warnings
    for (id, text) in &scripts {
        match core::preprocess(text, &core::PreprocessOptions::default()) {
            Ok(pre) => {
                pre_maps.insert(id.clone(), pre.sourcemap);
                pre_scripts.insert(id.clone(), pre.text);
                for d in pre.diags {
                    out.push(WasmDiagnostic {
                        severity: match d.severity {
                            core::Severity::Warning => "warning".into(),
                            core::Severity::Error => "error".into(),
                        },
                        line: d.line,
                        col: d.col,
                        span_len: d.span_len,
                        code: d.code.into(),
                        message: d.message,
                        suggestion: d.suggestion,
                        script: Some(id.clone()),
                    });
                }
            }
            Err(e) => {
                out.push(WasmDiagnostic {
                    severity: "error".into(),
                    line: e.line,
                    col: e.col,
                    span_len: 0,
                    code: e.code.into(),
                    message: e.message,
                    suggestion: None,
                    script: Some(id.clone()),
                });
            }
        }
    }

    // 3) Parse errors in entry and scripts (with mapping to original lines)
    if let Some(ref txt) = entry_pre_text {
        let exists = |id: &str| scripts.get(id).is_some();
        if let Err(e) = core::compile_dsl_with_diag(txt, exists) {
            let mapped_line = entry_pre_map
                .as_ref()
                .and_then(|m| m.get((e.line as usize).saturating_sub(1)))
                .map(|loc| loc.line)
                .unwrap_or(e.line);
            out.push(WasmDiagnostic {
                severity: "error".into(),
                line: mapped_line,
                col: 0,
                span_len: 0,
                code: match e.kind {
                    DslError::TooManyLines => "TooManyLines".into(),
                    DslError::UnknownCommand => "UnknownCommand".into(),
                    DslError::InvalidLine => "InvalidLine".into(),
                    DslError::ParseKey => "ParseKey".into(),
                    DslError::ParseMod => "ParseMod".into(),
                    DslError::ParseDelay => "ParseDelay".into(),
                    DslError::TextEmpty => "TextEmpty".into(),
                    DslError::UnknownScript => "UnknownScript".into(),
                    DslError::RecursionTooDeep => "RecursionTooDeep".into(),
                },
                message: format!("parse error at line {}", mapped_line),
                suggestion: None,
                script: None,
            });
        }
    }
    for (id, txt) in &pre_scripts {
        if let Err(e) = core::compile_dsl_with_diag(txt, |sid| scripts.get(sid).is_some()) {
            let mapped_line = pre_maps
                .get(id)
                .and_then(|m| m.get((e.line as usize).saturating_sub(1)))
                .map(|loc| loc.line)
                .unwrap_or(e.line);
            out.push(WasmDiagnostic {
                severity: "error".into(),
                line: mapped_line,
                col: 0,
                span_len: 0,
                code: match e.kind {
                    DslError::TooManyLines => "TooManyLines".into(),
                    DslError::UnknownCommand => "UnknownCommand".into(),
                    DslError::InvalidLine => "InvalidLine".into(),
                    DslError::ParseKey => "ParseKey".into(),
                    DslError::ParseMod => "ParseMod".into(),
                    DslError::ParseDelay => "ParseDelay".into(),
                    DslError::TextEmpty => "TextEmpty".into(),
                    DslError::UnknownScript => "UnknownScript".into(),
                    DslError::RecursionTooDeep => "RecursionTooDeep".into(),
                },
                message: format!("parse error at line {}", mapped_line),
                suggestion: None,
                script: Some(id.clone()),
            });
        }
    }

    // 4) Attempt compile & link to surface link-time errors
    let provider = |id: &str| scripts.get(id).map(|s| s.as_str());
    if let Err(e) = core::compile_and_link(entry_dsl, &provider) {
        out.push(WasmDiagnostic {
            severity: "error".into(),
            line: e.line,
            col: 0,
            span_len: 0,
            code: match e.kind {
                DslError::TooManyLines => "TooManyLines".into(),
                DslError::UnknownCommand => "UnknownCommand".into(),
                DslError::InvalidLine => "InvalidLine".into(),
                DslError::ParseKey => "ParseKey".into(),
                DslError::ParseMod => "ParseMod".into(),
                DslError::ParseDelay => "ParseDelay".into(),
                DslError::TextEmpty => "TextEmpty".into(),
                DslError::UnknownScript => "UnknownScript".into(),
                DslError::RecursionTooDeep => "RecursionTooDeep".into(),
            },
            message: format!("parse/link error at line {}", e.line),
            suggestion: None,
            script: None,
        });
    }

    // 5) Unused scripts (reachable analysis via AST walk)
    let mut reachable: HashSet<String> = HashSet::new();
    // BFS from entry
    let mut q: VecDeque<String> = VecDeque::new();
    // Compile entry (preprocessed if available) to AST with existence check
    let entry_src: std::borrow::Cow<'_, str> = match entry_pre_text {
        Some(ref s) => std::borrow::Cow::Borrowed(s.as_str()),
        None => std::borrow::Cow::Borrowed(entry_dsl),
    };
    let exists = |id: &str| scripts.get(id).is_some();
    if let Ok(ast) = core::compile_dsl_with_diag(&entry_src, exists) {
        for op in ast.ops {
            if let dsl_core::Op::Call { id } = op {
                if !reachable.contains(id) {
                    reachable.insert(id.to_string());
                    q.push_back(id.to_string());
                }
            }
        }
        while let Some(id) = q.pop_front() {
            if let Some(text) = pre_scripts.get(&id).or_else(|| scripts.get(&id)) {
                if let Ok(ast2) =
                    core::compile_dsl_with_diag(text, |sid| scripts.get(sid).is_some())
                {
                    for op in ast2.ops {
                        if let dsl_core::Op::Call { id: sub } = op {
                            if reachable.insert(sub.to_string()) {
                                q.push_back(sub.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    for key in scripts.keys() {
        if !reachable.contains(key) {
            out.push(WasmDiagnostic {
                severity: "warning".into(),
                line: 0,
                col: 0,
                span_len: 0,
                code: "UnusedScript".into(),
                message: format!("script '{}' is not referenced by entry or its callees", key),
                suggestion: None,
                script: Some(key.clone()),
            });
        }
    }

    JsValue::from_serde(&out).map_err(|e| JsValue::from_str(&e.to_string()))
}
