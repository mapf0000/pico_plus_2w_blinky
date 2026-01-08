use dsl_core::{self as core, CompileError, Severity, Span};
use std::collections::{HashMap, HashSet, VecDeque};
use std::str::FromStr;
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

fn diag(err: CompileError) -> JsValue {
    let message = if err.message.is_empty() {
        err.code.to_string()
    } else {
        err.message
    };
    let wd = WasmDiagnostic {
        severity: severity_label(err.severity).into(),
        line: err.span.line,
        col: err.span.col,
        span_len: err.span.len,
        code: err.code.into(),
        message,
        suggestion: err.suggestion,
        script: None,
    };
    JsValue::from_serde(&wd).unwrap_or_else(|_| JsValue::from_str(&format!("{:?}", wd)))
}

fn severity_label(sev: Severity) -> &'static str {
    match sev {
        Severity::Warning => "warning",
        Severity::Error => "error",
    }
}

fn remap_span(span: Span, map: &[core::OrigLoc]) -> Span {
    let idx = (span.line as usize).saturating_sub(1);
    if let Some(loc) = map.get(idx) {
        Span::new(loc.line, span.col, span.len)
    } else {
        span
    }
}

/// Compile a DSL entry script and a set of named scripts (by JSON id→text)
/// into bytecode suitable for the Pi executor.
/// - `scripts_json`: a JSON object like {"hello":"text ...", "foo":"..."}
#[wasm_bindgen]
pub fn compile_to_bytecode(entry_dsl: &str, scripts_json: &str) -> Result<Box<[u8]>, JsValue> {
    compile_to_bytecode_with_layout(entry_dsl, scripts_json, core::DEFAULT_LAYOUT_ID)
}

/// Same as compile_to_bytecode, but uses a caller-provided default layout id.
#[wasm_bindgen]
pub fn compile_to_bytecode_with_layout(
    entry_dsl: &str,
    scripts_json: &str,
    default_layout: &str,
) -> Result<Box<[u8]>, JsValue> {
    let scripts: HashMap<String, String> =
        serde_json::from_str(scripts_json).map_err(|e| JsValue::from_str(&e.to_string()))?;

    let provider = |id: &str| scripts.get(id).map(|s| s.as_str());

    // 1) compile + link (inline calls)
    let owned = core::compile_and_link(entry_dsl, &provider).map_err(diag)?;

    let layout = core::LayoutId::from_str(default_layout).map_err(|err| {
        let msg = match err {
            core::LayoutParseError::Unknown => "unknown default layout",
            core::LayoutParseError::NotEnabled => "default layout not enabled",
        };
        JsValue::from_str(&format!("{msg}: {default_layout}"))
    })?;

    // 2) lower to flat ops using layout
    let flat = core::lower_to_flat_with_layout(&owned, layout).map_err(diag)?;

    // 3) encode to bytecode
    let bytes = core::bytecode::encode(&flat)
        .map_err(|_| JsValue::from_str("program exceeds maximum length"))?;
    Ok(bytes.into_boxed_slice())
}

/// Quick syntax check of a single DSL text (no linking, no lowering).
#[wasm_bindgen]
pub fn lint_dsl(dsl: &str) -> Result<(), JsValue> {
    let pre = core::preprocess(dsl, &core::PreprocessOptions::default()).map_err(diag)?;
    let accept_all = |_| true;
    match core::compile_dsl_with_diag(&pre.text, accept_all) {
        Ok(_) => Ok(()),
        Err(e) => {
            let mut err = e;
            err.span = remap_span(err.span, &pre.sourcemap);
            Err(diag(err))
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
            for d in pre.diagnostics {
                out.push(WasmDiagnostic {
                    severity: severity_label(d.severity).into(),
                    line: d.span.line,
                    col: d.span.col,
                    span_len: d.span.len,
                    code: d.code.into(),
                    message: d.message,
                    suggestion: d.suggestion,
                    script: None,
                });
            }
        }
        Err(e) => {
            out.push(WasmDiagnostic {
                severity: severity_label(e.severity).into(),
                line: e.span.line,
                col: e.span.col,
                span_len: e.span.len,
                code: e.code.into(),
                message: e.message,
                suggestion: e.suggestion,
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
                for d in pre.diagnostics {
                    out.push(WasmDiagnostic {
                        severity: severity_label(d.severity).into(),
                        line: d.span.line,
                        col: d.span.col,
                        span_len: d.span.len,
                        code: d.code.into(),
                        message: d.message,
                        suggestion: d.suggestion,
                        script: Some(id.clone()),
                    });
                }
            }
            Err(e) => {
                out.push(WasmDiagnostic {
                    severity: severity_label(e.severity).into(),
                    line: e.span.line,
                    col: e.span.col,
                    span_len: e.span.len,
                    code: e.code.into(),
                    message: e.message,
                    suggestion: e.suggestion,
                    script: Some(id.clone()),
                });
            }
        }
    }

    // 3) Parse errors in entry and scripts (with mapping to original lines)
    if let Some(ref txt) = entry_pre_text {
        let exists = |id: &str| scripts.get(id).is_some();
        if let Err(e) = core::compile_dsl_with_diag(txt, exists) {
            let mut err = e;
            if let Some(map) = entry_pre_map.as_ref() {
                err.span = remap_span(err.span, map);
            }
            out.push(WasmDiagnostic {
                severity: severity_label(err.severity).into(),
                line: err.span.line,
                col: err.span.col,
                span_len: err.span.len,
                code: err.code.into(),
                message: err.message,
                suggestion: err.suggestion,
                script: None,
            });
        }
    }
    for (id, txt) in &pre_scripts {
        if let Err(e) = core::compile_dsl_with_diag(txt, |sid| scripts.get(sid).is_some()) {
            let mut err = e;
            if let Some(map) = pre_maps.get(id) {
                err.span = remap_span(err.span, map);
            }
            out.push(WasmDiagnostic {
                severity: severity_label(err.severity).into(),
                line: err.span.line,
                col: err.span.col,
                span_len: err.span.len,
                code: err.code.into(),
                message: err.message,
                suggestion: err.suggestion,
                script: Some(id.clone()),
            });
        }
    }

    // 4) Attempt compile & link to surface link-time errors
    let provider = |id: &str| scripts.get(id).map(|s| s.as_str());
    if let Err(e) = core::compile_and_link(entry_dsl, &provider) {
        out.push(WasmDiagnostic {
            severity: severity_label(e.severity).into(),
            line: e.span.line,
            col: e.span.col,
            span_len: e.span.len,
            code: e.code.into(),
            message: e.message,
            suggestion: e.suggestion,
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
