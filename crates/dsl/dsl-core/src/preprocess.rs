#[cfg(feature = "std")]
use std::{collections::BTreeMap, format, string::String, string::ToString, vec::Vec};

#[cfg(not(feature = "std"))]
use alloc::{collections::BTreeMap, format, string::String, string::ToString, vec::Vec};

use crate::limits::{MAX_DSL_LINES, MAX_EXPANDED_LINES, MAX_REPEAT_N};
use crate::{CompileError, Span};

// -------- Phase 1 Preprocessor (repeat, let, limited lints) --------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrigLoc {
    pub line: u16,
    pub col: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreprocessOutput {
    pub text: String,
    pub sourcemap: Vec<OrigLoc>,
    pub diagnostics: Vec<CompileError>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreprocessOptions {
    pub max_repeat_n: u32,
    pub max_expanded_lines: usize,
    pub near_cap_ratio: f32,
}
impl Default for PreprocessOptions {
    fn default() -> Self {
        Self {
            max_repeat_n: MAX_REPEAT_N,
            max_expanded_lines: MAX_EXPANDED_LINES,
            near_cap_ratio: 0.9,
        }
    }
}

fn pre_error(code: &'static str, message: impl Into<String>, line: u16, col: u16) -> CompileError {
    CompileError::error(code, message, Span::new(line, col, 0))
}

fn pre_warning(
    code: &'static str,
    message: impl Into<String>,
    line: u16,
    col: u16,
    span_len: u16,
    suggestion: Option<String>,
) -> CompileError {
    CompileError::warning(code, message, Span::new(line, col, span_len), suggestion)
}

pub(crate) fn map_pre_span(span: Span, sm: &[OrigLoc]) -> Span {
    let idx = (span.line as usize).saturating_sub(1);
    if idx < sm.len() {
        Span::new(sm[idx].line, span.col, span.len)
    } else {
        span
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConstVal {
    Str(String),
    Num(u64),
}

#[derive(Debug, Default)]
struct LetEnv {
    items: Vec<(String, ConstVal)>,
}
impl LetEnv {
    fn get(&self, name: &str) -> Option<&ConstVal> {
        self.items
            .iter()
            .rev()
            .find(|(n, _)| n.as_str() == name)
            .map(|(_, v)| v)
    }
    fn insert(&mut self, name: String, val: ConstVal) -> Result<(), ()> {
        if self.items.iter().any(|(n, _)| n == &name) {
            return Err(());
        }
        self.items.push((name, val));
        Ok(())
    }
    fn insert_shadow(&mut self, name: String, val: ConstVal) {
        if let Some((_, existing)) = self.items.iter_mut().find(|(n, _)| n == &name) {
            *existing = val;
        } else {
            self.items.push((name, val));
        }
    }
    fn clone_from_parent(parent: &LetEnv) -> Self {
        Self {
            items: parent.items.clone(),
        }
    }
}

type FunctionRegistry = BTreeMap<String, FunctionDef>;

#[derive(Debug, Clone)]
struct FunctionDef {
    body_start: usize,
    body_end: usize,
    defined_line: u16,
    defined_col: u16,
    params: Vec<String>,
    used: bool,
}

#[derive(Debug, Default)]
struct LegacyHintFlags {
    delay: bool,
    text: bool,
    modtap: bool,
    repeat: bool,
    tap: bool,
}

struct ExpansionContext<'a, 'src> {
    lines: &'a [&'src str],
    opts: &'a PreprocessOptions,
    functions: &'a mut FunctionRegistry,
    stack: &'a mut Vec<String>,
    hints: &'a mut LegacyHintFlags,
}

struct EmitOutputs<'a> {
    lines: &'a mut Vec<String>,
    source_map: &'a mut Vec<OrigLoc>,
    diagnostics: &'a mut Vec<CompileError>,
    hints: &'a mut LegacyHintFlags,
}

pub fn preprocess(src: &str, opts: &PreprocessOptions) -> Result<PreprocessOutput, CompileError> {
    let mut out_lines: Vec<String> = Vec::new();
    let mut sm: Vec<OrigLoc> = Vec::new();
    let mut diags: Vec<CompileError> = Vec::new();
    let mut env = LetEnv::default();
    let mut hints = LegacyHintFlags::default();

    let mut lines: Vec<&str> = Vec::new();
    for l in src.split('\n') {
        lines.push(l);
    }

    let (mut functions, skip_ranges) = gather_functions(&lines)?;
    let mut skip_idx = 0usize;
    let mut current_skip = skip_ranges.get(skip_idx).copied();
    let mut fn_stack: Vec<String> = Vec::new();

    let mut i = 0usize;
    while i < lines.len() {
        if let Some((start, end)) = current_skip
            && i >= start
            && i <= end
        {
            if i == end {
                skip_idx += 1;
                current_skip = skip_ranges.get(skip_idx).copied();
            }
            i += 1;
            continue;
        }

        let raw = lines[i];
        let trimmed = raw.trim();
        let line_no = (i + 1) as u16;
        if trimmed.is_empty() || trimmed.starts_with('#') {
            i += 1;
            continue;
        }

        if let Some(rest) = starts_with_ci(trimmed, "let") {
            match parse_let(rest) {
                Ok((name, val)) => {
                    if !is_upper_name(&name) {
                        return Err(pre_error(
                            "LetInvalidName",
                            format!("invalid constant name '{}': must be [A-Z_][A-Z0-9_]*", name),
                            line_no,
                            1,
                        ));
                    }
                    if env.insert(name, val).is_err() {
                        return Err(pre_error(
                            "LetRedefinition",
                            "constant already defined",
                            line_no,
                            1,
                        ));
                    }
                }
                Err(pe) => {
                    return Err(pre_error(pe.0, pe.1, line_no, 1));
                }
            }
            i += 1;
            continue;
        }

        if let Some(rest) = repeat_rest(trimmed) {
            let trimmed_ws = trimmed.trim_start();
            if !hints.repeat
                && trimmed_ws.len() > 6
                && trimmed_ws[..6].eq_ignore_ascii_case("repeat")
                && trimmed_ws.as_bytes()[6].is_ascii_whitespace()
            {
                diags.push(pre_warning(
                    "LegacyRepeatSyntax",
                    "repeat blocks support the function form repeat(N) { ... }",
                    line_no,
                    1,
                    0,
                    Some("Rewrite as repeat(N) { ... }".into()),
                ));
                hints.repeat = true;
            }
            let (n, has_brace) = match parse_repeat_header(rest) {
                Ok((n, b)) => (n, b),
                Err((code, msg)) => {
                    return Err(pre_error(code, msg, line_no, 1));
                }
            };
            if !has_brace {
                return Err(pre_error(
                    "RepeatMissingBrace",
                    "expected '{' after repeat N",
                    line_no,
                    1,
                ));
            }
            if n > opts.max_repeat_n {
                return Err(pre_error(
                    "RepeatNTooLarge",
                    format!("repeat count {} exceeds cap {}", n, opts.max_repeat_n),
                    line_no,
                    1,
                ));
            }

            let body_start = i + 1;
            let mut depth: i32 = 1;
            let mut j = i + 1;
            while j < lines.len() {
                if let Some((s, e)) = current_skip
                    && j >= s
                    && j <= e
                {
                    if j == e {
                        skip_idx += 1;
                        current_skip = skip_ranges.get(skip_idx).copied();
                    }
                    j += 1;
                    continue;
                }
                let t = lines[j].trim();
                if t.is_empty() || t.starts_with('#') {
                    j += 1;
                    continue;
                }
                if let Some(r2) = repeat_rest(t) {
                    if let Ok((_cn, has)) = parse_repeat_header(r2)
                        && has
                    {
                        depth += 1;
                    }
                } else if t == "}" {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                j += 1;
            }
            if depth != 0 {
                return Err(pre_error(
                    "RepeatMissingBrace",
                    "missing closing '}' for repeat block",
                    line_no,
                    1,
                ));
            }
            let body_end = j;

            let child_env = LetEnv::clone_from_parent(&env);
            let body = preprocess_block(
                &mut ExpansionContext {
                    lines: &lines,
                    opts,
                    functions: &mut functions,
                    stack: &mut fn_stack,
                    hints: &mut hints,
                },
                body_start,
                body_end,
                child_env,
            )?;

            if out_lines.len() + body.lines.len().saturating_mul(n as usize)
                > opts.max_expanded_lines
            {
                return Err(pre_error(
                    "RepeatExpansionTooLarge",
                    "expanded lines exceed cap",
                    line_no,
                    1,
                ));
            }
            for _ in 0..n {
                append_block(&body, &mut out_lines, &mut sm);
            }

            i = j + 1;
            continue;
        }

        match parse_function_call_line(trimmed) {
            Ok(Some(call)) => {
                if call.name.eq_ignore_ascii_case("delay")
                    || call.name.eq_ignore_ascii_case("text")
                    || call.name.eq_ignore_ascii_case("layout")
                    || call.name.eq_ignore_ascii_case("modtap")
                    || call.name.eq_ignore_ascii_case("tap")
                {
                    // Built-in command handled later in substitute_and_emit.
                } else {
                    let args = parse_call_args(call.args, &env)
                        .map_err(|(code, msg)| pre_error(code, msg, line_no, 1))?;
                    expand_function(
                        call.name,
                        &args,
                        line_no,
                        &env,
                        &mut ExpansionContext {
                            lines: &lines,
                            opts,
                            functions: &mut functions,
                            stack: &mut fn_stack,
                            hints: &mut hints,
                        },
                        &mut out_lines,
                        &mut sm,
                    )?;
                    if out_lines.len() > opts.max_expanded_lines {
                        return Err(pre_error(
                            "ExpandedLinesTooLarge",
                            "expanded lines exceed cap",
                            line_no,
                            1,
                        ));
                    }
                    i += 1;
                    continue;
                }
            }
            Ok(None) => {}
            Err((code, msg)) => {
                return Err(pre_error(code, msg, line_no, 1));
            }
        }

        if trimmed == "}" {
            return Err(pre_error("UnexpectedBrace", "unexpected '}'", line_no, 1));
        }

        match substitute_and_emit(
            trimmed,
            line_no,
            &env,
            &mut EmitOutputs {
                lines: &mut out_lines,
                source_map: &mut sm,
                diagnostics: &mut diags,
                hints: &mut hints,
            },
            true,
        ) {
            Ok(()) => {}
            Err(pe) => {
                return Err(pre_error(pe.0, pe.1, line_no, 1));
            }
        }
        if out_lines.len() > opts.max_expanded_lines {
            return Err(pre_error(
                "ExpandedLinesTooLarge",
                "expanded lines exceed cap",
                line_no,
                1,
            ));
        }
        i += 1;
    }

    for (name, def) in &functions {
        if !def.used {
            diags.push(pre_warning(
                "FnUnused",
                format!("function '{}' is defined but never called", name),
                def.defined_line,
                def.defined_col,
                0,
                None,
            ));
        }
    }

    let approx_nonempty = out_lines.len();
    if approx_nonempty as f32 >= (MAX_DSL_LINES as f32 * opts.near_cap_ratio) {
        diags.push(pre_warning(
            "NearCapLines",
            format!(
                "lines approaching cap: {} / {}",
                approx_nonempty, MAX_DSL_LINES
            ),
            0,
            0,
            0,
            None,
        ));
    }

    Ok(PreprocessOutput {
        text: join_lines(&out_lines),
        sourcemap: sm,
        diagnostics: diags,
    })
}

#[derive(Debug)]
struct PreBlock {
    lines: Vec<String>,
    map: Vec<OrigLoc>,
}

fn preprocess_block(
    context: &mut ExpansionContext<'_, '_>,
    start: usize,
    end: usize,
    mut env: LetEnv,
) -> Result<PreBlock, CompileError> {
    let mut out: Vec<String> = Vec::new();
    let mut sm: Vec<OrigLoc> = Vec::new();
    let mut diags: Vec<CompileError> = Vec::new();
    let mut i = start;
    while i < end {
        let raw = context.lines[i];
        let t = raw.trim();
        let line_no = (i + 1) as u16;
        if t.is_empty() || t.starts_with('#') {
            i += 1;
            continue;
        }
        if let Some(rest) = starts_with_ci(t, "let") {
            match parse_let(rest) {
                Ok((name, val)) => {
                    if !is_upper_name(&name) {
                        return Err(pre_error(
                            "LetInvalidName",
                            format!("invalid constant name '{}': must be [A-Z_][A-Z0-9_]*", name),
                            line_no,
                            1,
                        ));
                    }
                    if env.insert(name, val).is_err() {
                        return Err(pre_error(
                            "LetRedefinition",
                            "constant already defined",
                            line_no,
                            1,
                        ));
                    }
                }
                Err(pe) => {
                    return Err(pre_error(pe.0, pe.1, line_no, 1));
                }
            }
            i += 1;
            continue;
        }
        if let Some(rest) = repeat_rest(t) {
            let (n, has) = match parse_repeat_header(rest) {
                Ok(v) => v,
                Err((c, m)) => {
                    return Err(pre_error(c, m, line_no, 1));
                }
            };
            if !has {
                return Err(pre_error(
                    "RepeatMissingBrace",
                    "expected '{' after repeat N",
                    line_no,
                    1,
                ));
            }
            if n > context.opts.max_repeat_n {
                return Err(pre_error(
                    "RepeatNTooLarge",
                    format!(
                        "repeat count {} exceeds cap {}",
                        n, context.opts.max_repeat_n
                    ),
                    line_no,
                    1,
                ));
            }
            // Find matching '}'
            let mut depth: i32 = 1;
            let mut j = i + 1;
            let body_start = i + 1;
            while j < end {
                let tt = context.lines[j].trim();
                if tt.is_empty() || tt.starts_with('#') {
                    j += 1;
                    continue;
                }
                if let Some(r2) = repeat_rest(tt) {
                    if let Ok((_cn, hb)) = parse_repeat_header(r2)
                        && hb
                    {
                        depth += 1;
                    }
                } else if tt == "}" {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                j += 1;
            }
            if depth != 0 {
                return Err(pre_error(
                    "RepeatMissingBrace",
                    "missing closing '}' for repeat block",
                    line_no,
                    1,
                ));
            }
            let body = preprocess_block(context, body_start, j, LetEnv::clone_from_parent(&env))?;
            if out.len() + body.lines.len().saturating_mul(n as usize)
                > context.opts.max_expanded_lines
            {
                return Err(pre_error(
                    "RepeatExpansionTooLarge",
                    "expanded lines exceed cap",
                    line_no,
                    1,
                ));
            }
            for _ in 0..n {
                append_block(&body, &mut out, &mut sm);
            }
            i = j + 1;
            continue;
        }
        match parse_function_call_line(t) {
            Ok(Some(call)) => {
                if call.name.eq_ignore_ascii_case("delay")
                    || call.name.eq_ignore_ascii_case("text")
                    || call.name.eq_ignore_ascii_case("layout")
                    || call.name.eq_ignore_ascii_case("modtap")
                    || call.name.eq_ignore_ascii_case("tap")
                {
                    // Built-in command handled later in substitute_and_emit.
                } else {
                    let args = parse_call_args(call.args, &env)
                        .map_err(|(code, msg)| pre_error(code, msg, line_no, 1))?;
                    expand_function(call.name, &args, line_no, &env, context, &mut out, &mut sm)?;
                    if out.len() > context.opts.max_expanded_lines {
                        return Err(pre_error(
                            "ExpandedLinesTooLarge",
                            "expanded lines exceed cap",
                            line_no,
                            1,
                        ));
                    }
                    i += 1;
                    continue;
                }
            }
            Ok(None) => {}
            Err((code, msg)) => {
                return Err(pre_error(code, msg, line_no, 1));
            }
        }
        // Regular line
        match substitute_and_emit(
            t,
            line_no,
            &env,
            &mut EmitOutputs {
                lines: &mut out,
                source_map: &mut sm,
                diagnostics: &mut diags,
                hints: context.hints,
            },
            true,
        ) {
            Ok(()) => {}
            Err(pe) => {
                return Err(pre_error(pe.0, pe.1, line_no, 1));
            }
        }
        if out.len() > context.opts.max_expanded_lines {
            return Err(pre_error(
                "ExpandedLinesTooLarge",
                "expanded lines exceed cap",
                line_no,
                1,
            ));
        }
        i += 1;
    }
    Ok(PreBlock {
        lines: out,
        map: sm,
    })
}

fn append_block(b: &PreBlock, out_lines: &mut Vec<String>, sm: &mut Vec<OrigLoc>) {
    for (k, l) in b.lines.iter().enumerate() {
        out_lines.push(l.clone());
        sm.push(b.map.get(k).copied().unwrap_or(OrigLoc { line: 0, col: 0 }));
    }
}

fn join_lines(lines: &[String]) -> String {
    let mut out = String::new();
    for (i, l) in lines.iter().enumerate() {
        if i != 0 {
            out.push('\n');
        }
        out.push_str(l);
    }
    out
}

fn is_ident(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let mut it = bytes.iter();
    let b0 = *it.next().unwrap();
    if !b0.is_ascii_lowercase() && !b0.is_ascii_uppercase() && b0 != b'_' {
        return false;
    }
    for &b in it {
        if !b.is_ascii_lowercase() && !b.is_ascii_uppercase() && !b.is_ascii_digit() && b != b'_' {
            return false;
        }
    }
    true
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_lowercase() || b.is_ascii_uppercase() || b == b'_'
}

fn is_ident_continue(b: u8) -> bool {
    is_ident_start(b) || b.is_ascii_digit()
}

fn repeat_rest(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if trimmed.len() < 6 {
        return None;
    }
    if trimmed[..6].eq_ignore_ascii_case("repeat") {
        let rest = &trimmed[6..];
        if rest.is_empty() {
            return Some(rest);
        }
        let first = rest.chars().next().unwrap();
        if first.is_whitespace() || first == '(' {
            return Some(rest.trim_start());
        }
    }
    None
}

fn gather_functions(
    lines: &[&str],
) -> Result<(FunctionRegistry, Vec<(usize, usize)>), CompileError> {
    let mut functions: FunctionRegistry = BTreeMap::new();
    let mut skips: Vec<(usize, usize)> = Vec::new();
    let mut i = 0usize;
    let mut block_depth: i32 = 0;
    let mut block_stack: Vec<u16> = Vec::new();
    while i < lines.len() {
        let raw = lines[i];
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            i += 1;
            continue;
        }

        if trimmed == "}" {
            if block_depth == 0 {
                return Err(pre_error(
                    "UnexpectedBrace",
                    "unexpected '}'",
                    (i as u16) + 1,
                    1,
                ));
            }
            block_depth -= 1;
            block_stack.pop();
            i += 1;
            continue;
        }

        if let Some(rest) = repeat_rest(trimmed) {
            let (_n, has) =
                parse_repeat_header(rest).map_err(|(c, m)| pre_error(c, m, (i as u16) + 1, 1))?;
            if !has {
                return Err(pre_error(
                    "RepeatMissingBrace",
                    "expected '{' after repeat N",
                    (i as u16) + 1,
                    1,
                ));
            }
            block_depth += 1;
            block_stack.push((i as u16) + 1);
            i += 1;
            continue;
        }

        if let Some(rest) = starts_with_ci(trimmed, "fn") {
            if block_depth != 0 {
                return Err(pre_error(
                    "FnNested",
                    "function definitions must appear at top level",
                    (i as u16) + 1,
                    1,
                ));
            }
            let (name, params, has_brace) =
                parse_fn_header(rest).map_err(|(c, m)| pre_error(c, m, (i as u16) + 1, 1))?;
            if !has_brace {
                return Err(pre_error(
                    "FnMissingBrace",
                    "expected '{' after function name",
                    (i as u16) + 1,
                    1,
                ));
            }
            if functions.contains_key(&name) {
                return Err(pre_error(
                    "FnRedefinition",
                    format!("function '{}' already defined", name),
                    (i as u16) + 1,
                    1,
                ));
            }
            let mut depth: i32 = 1;
            let mut j = i + 1;
            while j < lines.len() {
                let raw_body = lines[j];
                let trimmed_body = raw_body.trim();
                if trimmed_body.is_empty() || trimmed_body.starts_with('#') {
                    j += 1;
                    continue;
                }
                if starts_with_ci(trimmed_body, "fn").is_some() {
                    return Err(pre_error(
                        "FnNested",
                        "function definitions cannot be nested",
                        (j as u16) + 1,
                        1,
                    ));
                }
                if let Some(rest2) = repeat_rest(trimmed_body) {
                    let (_n, has) = parse_repeat_header(rest2)
                        .map_err(|(c, m)| pre_error(c, m, (j as u16) + 1, 1))?;
                    if !has {
                        return Err(pre_error(
                            "RepeatMissingBrace",
                            "expected '{' after repeat N",
                            (j as u16) + 1,
                            1,
                        ));
                    }
                    depth += 1;
                    j += 1;
                    continue;
                }
                if trimmed_body == "}" {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    j += 1;
                    continue;
                }
                j += 1;
            }
            if depth != 0 {
                return Err(pre_error(
                    "FnMissingBrace",
                    "missing closing '}' for function",
                    (i as u16) + 1,
                    1,
                ));
            }
            let body_start = i + 1;
            let body_end = j;
            functions.insert(
                name,
                FunctionDef {
                    body_start,
                    body_end,
                    defined_line: (i as u16) + 1,
                    defined_col: 1,
                    params,
                    used: false,
                },
            );
            skips.push((i, j));
            i = j + 1;
            continue;
        }

        i += 1;
    }
    if block_depth != 0 {
        let line = block_stack.pop().unwrap_or(lines.len() as u16);
        return Err(pre_error(
            "RepeatMissingBrace",
            "missing closing '}'",
            line,
            1,
        ));
    }
    Ok((functions, skips))
}

fn expand_function(
    name: &str,
    args: &[ConstVal],
    call_line: u16,
    env: &LetEnv,
    context: &mut ExpansionContext<'_, '_>,
    out_lines: &mut Vec<String>,
    sm: &mut Vec<OrigLoc>,
) -> Result<(), CompileError> {
    if context.stack.iter().any(|n| n == name) {
        return Err(pre_error(
            "FnRecursion",
            format!("recursive call of '{}'", name),
            call_line,
            1,
        ));
    }

    let def = match context.functions.get(name) {
        Some(def) => def.clone(),
        None => {
            return Err(pre_error(
                "FnUndefined",
                format!("function '{}' not defined", name),
                call_line,
                1,
            ));
        }
    };

    if def.params.len() != args.len() {
        return Err(pre_error(
            "FnWrongArgCount",
            format!(
                "function '{}' expects {} argument(s), got {}",
                name,
                def.params.len(),
                args.len()
            ),
            call_line,
            1,
        ));
    }

    context.stack.push(name.to_string());
    let mut call_env = LetEnv::clone_from_parent(env);
    for (param, value) in def.params.iter().zip(args.iter()) {
        call_env.insert_shadow(param.clone(), value.clone());
    }

    let block = match preprocess_block(context, def.body_start, def.body_end, call_env) {
        Ok(b) => b,
        Err(e) => {
            context.stack.pop();
            return Err(e);
        }
    };
    context.stack.pop();

    if out_lines.len() + block.lines.len() > context.opts.max_expanded_lines {
        return Err(pre_error(
            "ExpandedLinesTooLarge",
            "expanded lines exceed cap",
            call_line,
            1,
        ));
    }

    append_block(&block, out_lines, sm);
    if let Some(def_mut) = context.functions.get_mut(name) {
        def_mut.used = true;
    }
    Ok(())
}

fn is_upper_name(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let mut it = bytes.iter();
    let b0 = *it.next().unwrap();
    if !b0.is_ascii_uppercase() && b0 != b'_' {
        return false;
    }
    for &b in it {
        if !b.is_ascii_uppercase() && !b.is_ascii_digit() && b != b'_' {
            return false;
        }
    }
    true
}

fn starts_with_ci<'a>(line: &'a str, kw: &str) -> Option<&'a str> {
    let mut it = line.splitn(2, char::is_whitespace);
    let head = it.next()?;
    if head.eq_ignore_ascii_case(kw) {
        Some(it.next().unwrap_or("").trim_start())
    } else {
        None
    }
}

fn parse_let(rest: &str) -> Result<(String, ConstVal), (&'static str, String)> {
    // rest: NAME = VALUE
    let mut parts = rest.splitn(2, '=');
    let lhs = parts.next().unwrap_or("").trim();
    let rhs = parts
        .next()
        .ok_or(("LetMissingEquals", "expected '='".into()))?
        .trim();
    if lhs.is_empty() {
        return Err(("LetInvalidName", "missing name".into()));
    }
    if rhs.is_empty() {
        return Err(("LetMissingValue", "missing value".into()));
    }
    if rhs.starts_with('"') {
        // string literal: consume until closing unescaped '"'
        if !rhs.ends_with('"') || rhs.len() < 2 {
            return Err(("InvalidString", "unterminated string literal".into()));
        }
        let inner = &rhs[1..rhs.len() - 1];
        let s = unescape_string(inner).map_err(|e| ("InvalidStringEscape", e))?;
        Ok((lhs.to_string(), ConstVal::Str(s)))
    } else {
        // number
        let n = rhs
            .parse::<u64>()
            .map_err(|_| ("LetValueNotNumber", "expected number".into()))?;
        Ok((lhs.to_string(), ConstVal::Num(n)))
    }
}

fn unescape_string(s: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut iter = s.chars();
    while let Some(ch) = iter.next() {
        if ch == '\\' {
            match iter.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('t') => out.push('\t'),
                Some('n') => return Err("\\n not allowed in Phase 1".into()),
                Some(other) => return Err(format!("unsupported escape \\{}", other)),
                None => return Err("dangling escape".into()),
            }
        } else {
            out.push(ch);
        }
    }
    Ok(out)
}

fn parse_repeat_header(rest: &str) -> Result<(u32, bool), (&'static str, String)> {
    let trimmed = rest.trim();
    if let Some(without_open) = trimmed.strip_prefix('(') {
        let close = without_open
            .find(')')
            .ok_or(("RepeatMissingParen", "missing ')' in repeat".into()))?
            + 1;
        let inner = trimmed[1..close].trim();
        if inner.is_empty() {
            return Err(("RepeatMissingCount", "missing count".into()));
        }
        let n = inner.parse::<u32>().map_err(|_| {
            (
                "RepeatCountNotNumber",
                "repeat count must be a number".into(),
            )
        })?;
        let remainder = trimmed[close + 1..].trim_start();
        let has_brace = remainder.starts_with('{');
        Ok((n, has_brace))
    } else {
        let mut it = trimmed.split_whitespace();
        let n_tok = it
            .next()
            .ok_or(("RepeatMissingCount", "missing count".into()))?;
        let n = n_tok.parse::<u32>().map_err(|_| {
            (
                "RepeatCountNotNumber",
                "repeat count must be a number".into(),
            )
        })?;
        let has_brace = trimmed.contains('{');
        Ok((n, has_brace))
    }
}

fn parse_fn_header(rest: &str) -> Result<(String, Vec<String>, bool), (&'static str, String)> {
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        return Err(("FnMissingName", "missing function name".into()));
    }
    let bytes = trimmed.as_bytes();
    if bytes.is_empty() {
        return Err(("FnMissingName", "missing function name".into()));
    }
    let mut idx = 0usize;
    if !is_ident_start(bytes[idx]) {
        return Err((
            "FnInvalidName",
            "function name must start with [A-Za-z_]".into(),
        ));
    }
    idx += 1;
    while idx < bytes.len() && is_ident_continue(bytes[idx]) {
        idx += 1;
    }
    let name = &trimmed[..idx];
    if !is_ident(name) {
        return Err((
            "FnInvalidName",
            format!(
                "invalid function name '{}': must be [A-Za-z_][A-Za-z0-9_]*",
                name
            ),
        ));
    }

    let mut remainder = trimmed[idx..].trim_start();
    let mut params: Vec<String> = Vec::new();
    if remainder.starts_with('(') {
        let mut depth = 0i32;
        let mut in_string = false;
        let mut escape = false;
        let mut close_idx: Option<usize> = None;
        for (off, ch) in remainder.char_indices() {
            match ch {
                '"' => {
                    if !escape {
                        in_string = !in_string;
                    }
                    escape = false;
                }
                '\\' => {
                    if in_string {
                        escape = !escape;
                    }
                }
                '(' if !in_string => {
                    depth += 1;
                }
                ')' if !in_string => {
                    depth -= 1;
                    if depth == 0 {
                        close_idx = Some(off);
                        break;
                    }
                }
                _ => {
                    escape = false;
                }
            }
        }
        let close = close_idx.ok_or(("FnMissingParen", "missing ')' in function header".into()))?;
        let inner = remainder[1..close].trim();
        if !inner.is_empty() {
            for raw in inner.split(',') {
                let param = raw.trim();
                if param.is_empty() {
                    return Err(("FnInvalidParam", "empty parameter name".into()));
                }
                if !is_upper_name(param) {
                    return Err((
                        "FnInvalidParam",
                        format!(
                            "invalid parameter name '{}': must be [A-Z_][A-Z0-9_]*",
                            param
                        ),
                    ));
                }
                if params.iter().any(|p| p == param) {
                    return Err((
                        "FnDuplicateParam",
                        format!("duplicate parameter '{}'", param),
                    ));
                }
                params.push(param.to_string());
            }
        }
        remainder = remainder[(close + 1)..].trim_start();
    }

    let has_brace = remainder.starts_with('{');
    Ok((name.to_string(), params, has_brace))
}

struct CallSyntax<'a> {
    name: &'a str,
    args: &'a str,
}

fn parse_function_call_line(line: &str) -> Result<Option<CallSyntax<'_>>, (&'static str, String)> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Ok(None);
    }
    let bytes = trimmed.as_bytes();
    if bytes.is_empty() || !is_ident_start(bytes[0]) {
        return Ok(None);
    }
    let mut idx = 1usize;
    while idx < bytes.len() && is_ident_continue(bytes[idx]) {
        idx += 1;
    }
    let name = &trimmed[..idx];
    if !is_ident(name) {
        return Ok(None);
    }
    let mut remainder = trimmed[idx..].trim_start();
    if !remainder.starts_with('(') {
        return Ok(None);
    }
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    let mut close_idx: Option<usize> = None;
    for (off, ch) in remainder.char_indices() {
        match ch {
            '"' => {
                if !escape {
                    in_string = !in_string;
                }
                escape = false;
            }
            '\\' if in_string => {
                escape = !escape;
            }
            '(' if !in_string => {
                depth += 1;
            }
            ')' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    close_idx = Some(off);
                    break;
                }
            }
            _ => {
                escape = false;
            }
        }
    }
    let close = close_idx.ok_or(("FnCallMissingParen", "missing ')' in function call".into()))?;
    let args = remainder[1..close].trim();
    remainder = remainder[(close + 1)..].trim_start();
    if !remainder.is_empty() {
        return Err((
            "FnCallTrailing",
            "unexpected tokens after function call".into(),
        ));
    }
    Ok(Some(CallSyntax { name, args }))
}

fn parse_call_args(args: &str, env: &LetEnv) -> Result<Vec<ConstVal>, (&'static str, String)> {
    let mut out: Vec<ConstVal> = Vec::new();
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return Ok(out);
    }
    let mut current = String::new();
    let mut in_string = false;
    let mut escape = false;
    for ch in trimmed.chars() {
        match ch {
            '"' => {
                if !escape {
                    in_string = !in_string;
                }
                current.push(ch);
                escape = false;
            }
            '\\' if in_string => {
                current.push(ch);
                escape = !escape;
                continue;
            }
            ',' if !in_string => {
                let token = current.trim();
                if token.is_empty() {
                    return Err(("FnArgEmpty", "missing argument value".into()));
                }
                out.push(parse_single_arg(token, env)?);
                current.clear();
                escape = false;
                continue;
            }
            _ => {
                current.push(ch);
                escape = false;
            }
        }
    }
    if in_string {
        return Err(("FnArgUnclosedString", "unterminated string literal".into()));
    }
    let token = current.trim();
    if token.is_empty() {
        return Err(("FnArgEmpty", "missing argument value".into()));
    }
    out.push(parse_single_arg(token, env)?);
    Ok(out)
}

fn parse_single_arg(token: &str, env: &LetEnv) -> Result<ConstVal, (&'static str, String)> {
    if token.starts_with('"') {
        if !token.ends_with('"') || token.len() < 2 {
            return Err(("FnArgString", "unterminated string literal".into()));
        }
        let inner = &token[1..token.len() - 1];
        let s = unescape_string(inner).map_err(|e| ("FnArgStringEscape", e))?;
        return Ok(ConstVal::Str(s));
    }
    if token.chars().all(|c| c.is_ascii_digit()) {
        let n = token
            .parse::<u64>()
            .map_err(|_| ("FnArgNumber", "argument must be a number".into()))?;
        return Ok(ConstVal::Num(n));
    }
    if is_upper_name(token) {
        if let Some(val) = env.get(token) {
            return Ok(val.clone());
        }
        return Err(("FnArgUndefined", format!("undefined constant '{}'", token)));
    }
    Err((
        "FnArgInvalid",
        format!(
            "invalid argument '{}': expected string, number, or constant",
            token
        ),
    ))
}

fn substitute_and_emit(
    trimmed: &str,
    orig_line: u16,
    env: &LetEnv,
    outputs: &mut EmitOutputs<'_>,
    allow_hint: bool,
) -> Result<(), (&'static str, String)> {
    let EmitOutputs {
        lines: out_lines,
        source_map: sm,
        diagnostics: diags,
        hints,
    } = outputs;

    if let Ok(Some(call)) = parse_function_call_line(trimmed) {
        if call.name.eq_ignore_ascii_case("delay") {
            let args = parse_call_args(call.args, env)?;
            if args.len() != 1 {
                return Err((
                    "DelayArgCount",
                    "delay() expects exactly one argument".into(),
                ));
            }
            let ms = match &args[0] {
                ConstVal::Num(n) => *n,
                ConstVal::Str(_) => {
                    return Err((
                        "DelayArgNotNumber",
                        "delay() expects a numeric argument".into(),
                    ));
                }
            };
            let normalized = format!("delay {}", ms);
            return substitute_and_emit(
                &normalized,
                orig_line,
                env,
                &mut EmitOutputs {
                    lines: out_lines,
                    source_map: sm,
                    diagnostics: diags,
                    hints,
                },
                false,
            );
        } else if call.name.eq_ignore_ascii_case("text") {
            let args = parse_call_args(call.args, env)?;
            if args.is_empty() || args.len() > 2 {
                return Err(("TextArgCount", "text() expects one or two arguments".into()));
            }
            let text_val = match &args[0] {
                ConstVal::Str(s) => s.clone(),
                ConstVal::Num(_) => {
                    return Err((
                        "TextArgNotString",
                        "text() expects a string as first argument".into(),
                    ));
                }
            };
            if text_val.is_empty() {
                return Err(("TextEmpty", "missing text".into()));
            }
            let mut out_line = String::from("text ");
            out_line.push_str(&text_val);
            if args.len() == 2 {
                let delay_val = match &args[1] {
                    ConstVal::Num(n) => *n,
                    ConstVal::Str(_) => {
                        return Err((
                            "TextDelayNotNumber",
                            "text() second argument must be numeric".into(),
                        ));
                    }
                };
                out_line.push(' ');
                out_line.push_str(&delay_val.to_string());
            }
            out_lines.push(out_line);
            sm.push(OrigLoc {
                line: orig_line,
                col: 1,
            });
            return Ok(());
        } else if call.name.eq_ignore_ascii_case("layout") {
            let args = parse_call_args(call.args, env)?;
            if args.len() != 1 {
                return Err((
                    "LayoutArgCount",
                    "layout() expects exactly one argument".into(),
                ));
            }
            let layout_val = match &args[0] {
                ConstVal::Str(s) => s.clone(),
                ConstVal::Num(_) => {
                    return Err((
                        "LayoutArgNotString",
                        "layout() expects a string argument".into(),
                    ));
                }
            };
            let normalized = format!("layout {}", layout_val);
            return substitute_and_emit(
                &normalized,
                orig_line,
                env,
                &mut EmitOutputs {
                    lines: out_lines,
                    source_map: sm,
                    diagnostics: diags,
                    hints,
                },
                false,
            );
        } else if call.name.eq_ignore_ascii_case("modtap") {
            let arg_string = match parse_call_args(call.args, env) {
                Ok(args) => {
                    if args.len() != 1 {
                        return Err((
                            "ModtapArgCount",
                            "modtap() expects exactly one argument".into(),
                        ));
                    }
                    match &args[0] {
                        ConstVal::Str(s) => s.clone(),
                        ConstVal::Num(_) => {
                            return Err((
                                "ModtapArgNotString",
                                "modtap() expects a string argument".into(),
                            ));
                        }
                    }
                }
                Err((code, msg)) => {
                    if code == "FnArgInvalid" {
                        let raw = call.args.trim();
                        if raw.is_empty() {
                            return Err((
                                "ModtapArgCount",
                                "modtap() expects exactly one argument".into(),
                            ));
                        }
                        raw.to_string()
                    } else {
                        return Err((code, msg));
                    }
                }
            };
            let normalized = format!("modtap {}", arg_string);
            return substitute_and_emit(
                &normalized,
                orig_line,
                env,
                &mut EmitOutputs {
                    lines: out_lines,
                    source_map: sm,
                    diagnostics: diags,
                    hints,
                },
                false,
            );
        } else if call.name.eq_ignore_ascii_case("tap") {
            let key_string = match parse_call_args(call.args, env) {
                Ok(args) => {
                    if args.len() != 1 {
                        return Err(("TapArgCount", "tap() expects exactly one argument".into()));
                    }
                    match &args[0] {
                        ConstVal::Str(s) => s.clone(),
                        ConstVal::Num(_) => {
                            return Err((
                                "TapArgNotString",
                                "tap() expects a string argument".into(),
                            ));
                        }
                    }
                }
                Err((code, msg)) => {
                    if code == "FnArgInvalid" {
                        let raw = call.args.trim();
                        if raw.is_empty() {
                            return Err((
                                "TapArgCount",
                                "tap() expects exactly one argument".into(),
                            ));
                        }
                        raw.to_string()
                    } else {
                        return Err((code, msg));
                    }
                }
            };
            let normalized = format!("tap {}", key_string);
            return substitute_and_emit(
                &normalized,
                orig_line,
                env,
                &mut EmitOutputs {
                    lines: out_lines,
                    source_map: sm,
                    diagnostics: diags,
                    hints,
                },
                false,
            );
        }
    }

    if allow_hint
        && starts_with_ci(trimmed, "modtap").is_some()
        && !trimmed.trim_start().starts_with("modtap(")
        && !hints.modtap
    {
        diags.push(pre_warning(
            "LegacyModtapSyntax",
            "modtap now supports the function form modtap(\"MOD+KEY\")",
            orig_line,
            1,
            0,
            Some("Use modtap(\"MOD+KEY\")".into()),
        ));
        hints.modtap = true;
    }

    if starts_with_ci(trimmed, "hold").is_some() || starts_with_ci(trimmed, "release").is_some() {
        diags.push(pre_warning(
            "HoldReleaseNotSupported",
            "hold/release require firmware update; try modtap/tap",
            orig_line,
            1,
            0,
            Some("Use modtap(\"MOD+KEY\") or tap(\"KEY\")".into()),
        ));
    }

    if let Some(rest) = starts_with_ci(trimmed, "delay") {
        if allow_hint && !hints.delay {
            diags.push(pre_warning(
                "LegacyDelaySyntax",
                "delay now supports the function form delay(ms)",
                orig_line,
                1,
                0,
                Some("Use delay(ms)".into()),
            ));
            hints.delay = true;
        }
        let tok = rest.trim();
        let val = if is_upper_name(tok) {
            match env.get(tok) {
                Some(ConstVal::Num(n)) => n.to_string(),
                Some(ConstVal::Str(_)) => {
                    return Err(("LetTypeMismatch", "delay expects a number".into()));
                }
                None => return Err(("LetUndefined", format!("undefined constant '{}'", tok))),
            }
        } else {
            tok.to_string()
        };
        if val.trim() == "0" {
            diags.push(pre_warning(
                "UselessDelayZero",
                "delay 0 is a no-op",
                orig_line,
                1,
                0,
                Some("Remove this line".into()),
            ));
        }
        let prev_is_delay = out_lines
            .last()
            .map(|l| l.trim_start().to_ascii_lowercase().starts_with("delay "))
            .unwrap_or(false);
        if prev_is_delay {
            diags.push(pre_warning(
                "AdjacentDelays",
                "adjacent delays will be coalesced",
                orig_line,
                1,
                0,
                Some("Combine into one delay".into()),
            ));
        }
        out_lines.push(format!("delay {}", val));
        sm.push(OrigLoc {
            line: orig_line,
            col: 1,
        });
        return Ok(());
    }

    if let Some(rest) = starts_with_ci(trimmed, "layout") {
        let tok = rest.trim();
        if tok.is_empty() {
            return Err(("LayoutEmpty", "layout expects an id".into()));
        }
        let val = if is_upper_name(tok) {
            match env.get(tok) {
                Some(ConstVal::Str(s)) => s.clone(),
                Some(ConstVal::Num(_)) => {
                    return Err(("LetTypeMismatch", "layout expects a string".into()));
                }
                None => return Err(("LetUndefined", format!("undefined constant '{}'", tok))),
            }
        } else {
            tok.to_string()
        };
        out_lines.push(format!("layout {}", val));
        sm.push(OrigLoc {
            line: orig_line,
            col: 1,
        });
        return Ok(());
    }

    if let Some(rest) = starts_with_ci(trimmed, "text") {
        let rest = rest.trim();
        if rest.is_empty() {
            return Err(("TextEmpty", "missing text".into()));
        }
        if allow_hint && !hints.text {
            diags.push(pre_warning(
                "LegacyTextSyntax",
                "text now supports the function form text(\"...\", delay)",
                orig_line,
                1,
                0,
                Some("Use text(\"...\", delay)".into()),
            ));
            hints.text = true;
        }
        let mut text_part = rest;
        let mut delay_part = "";
        if let Some(idx) = rest.rfind(char::is_whitespace) {
            let (lhs, rhs) = rest.split_at(idx);
            let maybe = rhs.trim();
            if !maybe.is_empty() {
                delay_part = maybe;
                text_part = lhs.trim_end();
            }
        }
        let mut new_text: String = String::new();
        if let Some((first, tail)) = split2(text_part) {
            if is_upper_name(first) {
                match env.get(first) {
                    Some(ConstVal::Str(s)) => {
                        new_text.push_str(s);
                        text_part = tail.trim_start();
                    }
                    Some(ConstVal::Num(_)) => {
                        return Err(("LetTypeMismatch", "text expects a string".into()));
                    }
                    None => {
                        return Err(("LetUndefined", format!("undefined constant '{}'", first)));
                    }
                }
            } else {
                new_text.push_str(text_part);
                text_part = "";
            }
        } else if is_upper_name(text_part) {
            match env.get(text_part) {
                Some(ConstVal::Str(s)) => {
                    new_text.push_str(s);
                    text_part = "";
                }
                Some(ConstVal::Num(_)) => {
                    return Err(("LetTypeMismatch", "text expects a string".into()));
                }
                None => {
                    return Err((
                        "LetUndefined",
                        format!("undefined constant '{}'", text_part),
                    ));
                }
            }
        } else {
            new_text.push_str(text_part);
            text_part = "";
        }
        if !text_part.is_empty() {
            if !new_text.is_empty() {
                new_text.push(' ');
            }
            new_text.push_str(text_part);
        }
        let mut out_line = String::new();
        out_line.push_str("text ");
        out_line.push_str(&new_text);
        if !delay_part.is_empty() {
            let dval = if is_upper_name(delay_part) {
                match env.get(delay_part) {
                    Some(ConstVal::Num(n)) => n.to_string(),
                    Some(ConstVal::Str(_)) => {
                        return Err(("LetTypeMismatch", "text delay expects a number".into()));
                    }
                    None => {
                        return Err((
                            "LetUndefined",
                            format!("undefined constant '{}'", delay_part),
                        ));
                    }
                }
            } else {
                delay_part.to_string()
            };
            out_line.push(' ');
            out_line.push_str(&dval);
        }
        out_lines.push(out_line);
        sm.push(OrigLoc {
            line: orig_line,
            col: 1,
        });
        return Ok(());
    }

    if allow_hint
        && starts_with_ci(trimmed, "modtap").is_some()
        && !trimmed.trim_start().starts_with("modtap(")
        && !hints.modtap
    {
        diags.push(pre_warning(
            "LegacyModtapSyntax",
            "modtap now supports the function form modtap(\"MOD+KEY\")",
            orig_line,
            1,
            0,
            Some("Use modtap(\"MOD+KEY\")".into()),
        ));
        hints.modtap = true;
    }
    if allow_hint
        && starts_with_ci(trimmed, "tap").is_some()
        && !trimmed.trim_start().starts_with("tap(")
        && !hints.tap
    {
        diags.push(pre_warning(
            "LegacyTapSyntax",
            "tap now supports the function form tap(\"KEY\")",
            orig_line,
            1,
            0,
            Some("Use tap(\"KEY\")".into()),
        ));
        hints.tap = true;
    }

    out_lines.push(trimmed.to_string());
    sm.push(OrigLoc {
        line: orig_line,
        col: 1,
    });
    Ok(())
}

fn split2(s: &str) -> Option<(&str, &str)> {
    let (a, b) = s.split_once(char::is_whitespace)?;

    Some((a, b))
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use crate::{
        FlatOp, KEY_DELETE, KEY_ENTER, KeyTap, MOD_LCTRL, Mods, OpOwned, Severity,
        compile_and_link, lower_to_flat_us,
    };

    fn empty_provider(_: &str) -> Option<&str> {
        None
    }

    #[test]
    fn repeat_unroll_simple() {
        let entry = "repeat(3) {\n  tap(\"A\")\n}";
        let owned = compile_and_link(entry, &empty_provider).expect("compile_and_link");
        let flat = lower_to_flat_us(&owned).expect("lower_to_flat_us");
        let taps = flat
            .ops
            .iter()
            .filter(|op| matches!(op, FlatOp::Tap { .. }))
            .count();
        assert_eq!(taps, 3);
    }

    #[test]
    fn repeat_unroll_nested() {
        let entry = "repeat(2) {\n  repeat(2) {\n    tap(\"A\")\n  }\n}";
        let owned = compile_and_link(entry, &empty_provider).expect("compile_and_link");
        let flat = lower_to_flat_us(&owned).expect("lower_to_flat_us");
        let taps = flat
            .ops
            .iter()
            .filter(|op| matches!(op, FlatOp::Tap { .. }))
            .count();
        assert_eq!(taps, 4);
    }

    #[test]
    fn let_number_in_delay() {
        let entry = "let D = 150\n delay(D)";
        let owned = compile_and_link(entry, &empty_provider).expect("compile_and_link");
        assert!(matches!(owned.ops.as_slice(), [OpOwned::DelayMs(150)]));
    }

    #[test]
    fn let_string_in_text() {
        let entry = "let S = \"Hi\"\n text(S, 5)";
        let owned = compile_and_link(entry, &empty_provider).expect("compile_and_link");
        match owned.ops.as_slice() {
            [OpOwned::Text { s, delay_ms }] => {
                assert_eq!(s, "Hi");
                assert_eq!(*delay_ms, 5);
            }
            other => panic!("unexpected ops: {:?}", other),
        }
    }

    #[test]
    fn repeat_missing_brace_errors() {
        let entry = "repeat(2) {\n tap(\"A\")\n"; // missing closing brace
        let err = compile_and_link(entry, &empty_provider).unwrap_err();
        assert_eq!(err.span.line, 1); // header line
        assert_eq!(err.code, "RepeatMissingBrace");
    }

    #[test]
    fn let_redefinition_errors() {
        let entry = "let A = 1\nlet A = 2\n tap(\"A\")";
        let err = compile_and_link(entry, &empty_provider).unwrap_err();
        assert_eq!(err.span.line, 2);
        assert_eq!(err.code, "LetRedefinition");
    }

    #[test]
    fn repeat_expansion_cap_errors() {
        // 100 * 100 * 1 line = 10_000 > MAX_EXPANDED_LINES (4096)
        let entry = "repeat(100) {\n  repeat(100) {\n    tap(\"A\")\n  }\n}";
        let err = compile_and_link(entry, &empty_provider).unwrap_err();
        // Error reported at the outer repeat header
        assert_eq!(err.span.line, 1);
        assert_eq!(err.code, "RepeatExpansionTooLarge");
    }

    #[test]
    fn sourcemap_maps_error_inside_repeat_body() {
        // 'zzz' is an unknown command on line 2; it repeats but we expect
        // the first error to map back to original line 2.
        let entry = "repeat 2 {\n  zzz\n}";
        let err = compile_and_link(entry, &empty_provider).unwrap_err();
        assert_eq!(err.span.line, 2);
        assert_eq!(err.code, "UnknownCommand");
    }

    #[test]
    fn nested_sourcemap_deep_error() {
        // Error occurs inside inner repeat body at original line 3
        let entry = "repeat 2 {\n  repeat 3 {\n    zzz\n  }\n}";
        let err = compile_and_link(entry, &empty_provider).unwrap_err();
        assert_eq!(err.span.line, 3);
        assert_eq!(err.code, "UnknownCommand");
    }

    #[test]
    fn redefinition_inside_nested_block_errors() {
        let entry = "let A = \"X\"\nrepeat(2) {\n  let A = \"Y\"\n  tap(\"A\")\n}";
        let err = compile_and_link(entry, &empty_provider).unwrap_err();
        // Error should point to the nested 'let A = "Y"' at original line 3
        assert_eq!(err.span.line, 3);
        assert_eq!(err.code, "LetRedefinition");
    }

    #[test]
    fn function_invocation_inlines_body() {
        let entry = "fn greet {\n  text Hi 15\n}\ngreet()";
        let owned = compile_and_link(entry, &empty_provider).expect("compile_and_link");
        match owned.ops.as_slice() {
            [OpOwned::Text { s, delay_ms }] => {
                assert_eq!(s, "Hi");
                assert_eq!(*delay_ms, 15);
            }
            other => panic!("unexpected ops: {:?}", other),
        }
    }

    #[test]
    fn function_nested_invocation_expands() {
        let entry = "fn base {\n  tap(\"A\")\n}\nfn wrapper {\n  base()\n}\nwrapper()";
        let owned = compile_and_link(entry, &empty_provider).expect("compile_and_link");
        let flat = lower_to_flat_us(&owned).expect("lower_to_flat_us");
        let taps = flat
            .ops
            .iter()
            .filter(|op| matches!(op, FlatOp::Tap { .. }))
            .count();
        assert_eq!(taps, 1);
    }

    #[test]
    fn function_recursion_detected() {
        let entry = "fn loop_fn {\n  loop_fn()\n}\nloop_fn()";
        let err = preprocess(entry, &PreprocessOptions::default()).unwrap_err();
        assert_eq!(err.code, "FnRecursion");
        assert_eq!(err.span.line, 2);
    }

    #[test]
    fn function_undefined_detected() {
        let entry = "missing()";
        let err = preprocess(entry, &PreprocessOptions::default()).unwrap_err();
        assert_eq!(err.code, "FnUndefined");
        assert_eq!(err.span.line, 1);
    }

    #[test]
    fn function_with_parameters_substitutes() {
        let entry = "fn greet(NAME) {\n  text NAME 10\n}\ngreet(\"Alice\")";
        let owned = compile_and_link(entry, &empty_provider).expect("compile_and_link");
        match owned.ops.as_slice() {
            [OpOwned::Text { s, delay_ms }] => {
                assert_eq!(s, "Alice");
                assert_eq!(*delay_ms, 10);
            }
            other => panic!("unexpected ops: {:?}", other),
        }
    }

    #[test]
    fn function_argument_from_constant() {
        let entry = "let MSG = \"Hi\"\nfn greet(NAME) {\n  text NAME 5\n}\ngreet(MSG)";
        let owned = compile_and_link(entry, &empty_provider).expect("compile_and_link");
        match owned.ops.as_slice() {
            [OpOwned::Text { s, delay_ms }] => {
                assert_eq!(s, "Hi");
                assert_eq!(*delay_ms, 5);
            }
            other => panic!("unexpected ops: {:?}", other),
        }
    }

    #[test]
    fn function_wrong_arity_errors() {
        let entry = "fn greet(NAME) {\n  text NAME\n}\ngreet()";
        let err = preprocess(entry, &PreprocessOptions::default()).unwrap_err();
        assert_eq!(err.code, "FnWrongArgCount");
        assert_eq!(err.span.line, 4);
    }

    #[test]
    fn delay_function_syntax() {
        let entry = "delay(150)";
        let owned = compile_and_link(entry, &empty_provider).expect("compile_and_link");
        assert!(matches!(owned.ops.as_slice(), [OpOwned::DelayMs(150)]));
    }

    #[test]
    fn tap_function_syntax() {
        let entry = "tap(\"ENTER\")";
        let owned = compile_and_link(entry, &empty_provider).expect("compile_and_link");
        match owned.ops.as_slice() {
            [OpOwned::Tap(KeyTap { usage, mods })] => {
                assert_eq!(*mods, Mods::empty());
                assert_eq!(*usage, KEY_ENTER);
            }
            other => panic!("unexpected ops: {:?}", other),
        }
    }

    #[test]
    fn text_function_syntax() {
        let entry = "text(\"Hello\", 25)";
        let owned = compile_and_link(entry, &empty_provider).expect("compile_and_link");
        match owned.ops.as_slice() {
            [OpOwned::Text { s, delay_ms }] => {
                assert_eq!(s, "Hello");
                assert_eq!(*delay_ms, 25);
            }
            other => panic!("unexpected ops: {:?}", other),
        }
    }

    #[test]
    fn modtap_function_syntax() {
        let entry = "modtap(\"LCTRL+DELETE\")";
        let owned = compile_and_link(entry, &empty_provider).expect("compile_and_link");
        match owned.ops.as_slice() {
            [OpOwned::Tap(KeyTap { usage, mods })] => {
                assert_eq!(*mods, MOD_LCTRL);
                assert_eq!(*usage, KEY_DELETE);
            }
            other => panic!("unexpected ops: {:?}", other),
        }
    }

    #[test]
    fn repeat_function_syntax() {
        let entry = "repeat(2) {\n  tap(\"A\")\n}";
        let owned = compile_and_link(entry, &empty_provider).expect("compile_and_link");
        let flat = lower_to_flat_us(&owned).expect("lower_to_flat_us");
        let taps = flat
            .ops
            .iter()
            .filter(|op| matches!(op, FlatOp::Tap { .. }))
            .count();
        assert_eq!(taps, 2);
    }

    #[test]
    fn function_unused_warns() {
        let entry = "fn helper {\n  tap(\"A\")\n}\ntap(\"B\")";
        let out = preprocess(entry, &PreprocessOptions::default()).expect("preprocess");
        assert!(
            out.diagnostics
                .iter()
                .any(|d| d.code == "FnUnused" && matches!(d.severity, Severity::Warning))
        );
    }
}
