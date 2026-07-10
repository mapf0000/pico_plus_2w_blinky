#[cfg(feature = "std")]
use std::{borrow::ToOwned, string::String, vec::Vec};

#[cfg(not(feature = "std"))]
use alloc::{borrow::ToOwned, string::String, vec::Vec};

use crate::limits::MAX_TOTAL_FLAT_OPS;
use crate::parser::compile_dsl_with_diag;
use crate::preprocess::{PreprocessOptions, PreprocessOutput, map_pre_span, preprocess};
use crate::{CompileError, Op, OpOwned, Program, ProgramOwned, Span, message_for_code};

/// Simple provider used during linking. Returns DSL text for a script id.
pub trait ScriptProvider {
    fn get<'a>(&'a self, id: &'a str) -> Option<&'a str>;
}

impl<F> ScriptProvider for F
where
    for<'a> F: Fn(&'a str) -> Option<&'a str>,
{
    fn get<'a>(&'a self, id: &'a str) -> Option<&'a str> {
        (self)(id)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CompileOptions {
    pub preprocess: PreprocessOptions,
    /// Require every entry and named script to declare its layout as its first
    /// command. Layout declarations in called scripts are then ignored so the
    /// caller's entry layout is inherited across the linked program.
    pub require_layout: bool,
}

#[derive(Debug, Clone)]
pub struct CompileOutput {
    pub program: ProgramOwned,
    pub diagnostics: Vec<CompileError>,
}

/// Preprocess, parse, and link `entry_dsl`, returning an owned program and diagnostics.
pub fn compile(
    entry_dsl: &str,
    provider: &impl ScriptProvider,
    opts: &CompileOptions,
) -> Result<CompileOutput, CompileError> {
    let pre = preprocess(entry_dsl, &opts.preprocess)?;
    let PreprocessOutput {
        text,
        sourcemap,
        diagnostics,
    } = pre;
    let exists = |id: &str| provider.get(id).is_some();
    let ast = match compile_dsl_with_diag(&text, exists) {
        Ok(p) => p,
        Err(e) => {
            let mapped = map_pre_span(e.span, &sourcemap);
            return Err(CompileError::error(e.code, e.message, mapped));
        }
    };
    if opts.require_layout {
        validate_leading_layout(&ast)?;
    }
    let mut out = ProgramOwned::new();
    let mut stack: Vec<String> = Vec::new();
    inline_into_owned(&ast, provider, opts, &mut out, &mut stack, true)?;
    Ok(CompileOutput {
        program: out,
        diagnostics,
    })
}

/// Compile `entry_dsl`, resolve & inline all `call`s using `provider`,
/// and return an **owned** program with no `Call` ops.
pub fn compile_and_link(
    entry_dsl: &str,
    provider: &impl ScriptProvider,
) -> Result<ProgramOwned, CompileError> {
    compile(entry_dsl, provider, &CompileOptions::default()).map(|out| out.program)
}

/// Compile and link a self-contained entry script whose first command declares
/// its keyboard layout. Every called script must also have a leading layout,
/// but inherits the entry script's layout when it is inlined.
pub fn compile_and_link_with_required_layout(
    entry_dsl: &str,
    provider: &impl ScriptProvider,
) -> Result<ProgramOwned, CompileError> {
    compile(
        entry_dsl,
        provider,
        &CompileOptions {
            require_layout: true,
            ..CompileOptions::default()
        },
    )
    .map(|out| out.program)
}

fn inline_into_owned(
    ast: &Program<'_>,
    provider: &impl ScriptProvider,
    opts: &CompileOptions,
    out: &mut ProgramOwned,
    stack: &mut Vec<String>,
    is_entry: bool,
) -> Result<(), CompileError> {
    for (idx, op) in ast.ops.iter().enumerate() {
        match op {
            Op::Tap(k) => out.ops.push(OpOwned::Tap(*k)),
            Op::DelayMs(ms) => {
                if *ms != 0 {
                    out.ops.push(OpOwned::DelayMs(*ms));
                }
            }
            Op::Text { s, delay_ms } => out.ops.push(OpOwned::Text {
                s: (*s).to_owned(),
                delay_ms: *delay_ms,
            }),
            Op::Layout(layout) => {
                if !opts.require_layout || is_entry {
                    out.ops.push(OpOwned::Layout(*layout));
                }
            }
            Op::Call { id } => {
                // cycle detection
                if stack.iter().any(|s| s == id) {
                    return Err(link_error("RecursionTooDeep", (idx as u16) + 1));
                }
                stack.push((*id).to_owned());
                let Some(text) = provider.get(id) else {
                    return Err(link_error("UnknownScript", (idx as u16) + 1));
                };
                // Preprocess callee independently (file-local scope), then parse and inline.
                let pre = preprocess(text, &opts.preprocess)?;
                let exists = |sid: &str| provider.get(sid).is_some();
                let sub = match compile_dsl_with_diag(&pre.text, exists) {
                    Ok(p) => p,
                    Err(e) => {
                        let mapped = map_pre_span(e.span, &pre.sourcemap);
                        return Err(CompileError::error(e.code, e.message, mapped));
                    }
                };
                if opts.require_layout {
                    validate_leading_layout(&sub)?;
                }
                inline_into_owned(&sub, provider, opts, out, stack, false)?;
                stack.pop();
            }
        }
        if out.ops.len() > MAX_TOTAL_FLAT_OPS {
            // reusing the cap for owned too
            return Err(link_error("TooManyLines", 0));
        }
    }
    Ok(())
}

pub fn validate_leading_layout(ast: &Program<'_>) -> Result<(), CompileError> {
    if !matches!(ast.ops.first(), Some(Op::Layout(_))) {
        return Err(link_error("LayoutRequired", 1));
    }
    if let Some((idx, _)) = ast
        .ops
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, op)| matches!(op, Op::Layout(_)))
    {
        return Err(link_error("LayoutMustBeFirst", (idx as u16) + 1));
    }
    Ok(())
}

fn link_error(code: &'static str, line: u16) -> CompileError {
    CompileError::error(code, message_for_code(code), Span::line(line))
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use crate::{PreprocessOptions, preprocess};

    struct EmptyProvider;

    impl ScriptProvider for EmptyProvider {
        fn get<'a>(&'a self, _id: &'a str) -> Option<&'a str> {
            None
        }
    }

    struct LookupProvider {
        entries: Vec<(&'static str, &'static str)>,
    }

    impl ScriptProvider for LookupProvider {
        fn get<'a>(&'a self, id: &'a str) -> Option<&'a str> {
            self.entries.iter().find(|(k, _)| *k == id).map(|(_, v)| *v)
        }
    }

    #[test]
    fn cross_script_error_maps_to_callee_line() {
        let entry = "call sub";
        let sub = "# sub\nzzz\n"; // error at line 2
        let provider = LookupProvider {
            entries: vec![("sub", sub)],
        };
        let err = compile_and_link(entry, &provider).unwrap_err();
        assert_eq!(err.span.line, 2);
        assert_eq!(err.code, "UnknownCommand");
    }

    #[test]
    fn nested_cross_script_error_maps_deep() {
        let entry = "call A";
        let a = "call B";
        let b = "zzz"; // error at line 1 in B
        let provider = LookupProvider {
            entries: vec![("A", a), ("B", b)],
        };
        let err = compile_and_link(entry, &provider).unwrap_err();
        assert_eq!(err.span.line, 1);
        assert_eq!(err.code, "UnknownCommand");
    }

    #[test]
    fn unknown_layout_errors() {
        let entry = "layout(\"win_xx-YY\")\ntext(\"a\")";
        let _ = preprocess(entry, &PreprocessOptions::default())
            .unwrap_or_else(|e| panic!("preprocess: {:?}", e));
        let err = compile_and_link(entry, &EmptyProvider).unwrap_err();
        assert_eq!(err.span.line, 1);
        assert!(err.code == "UnknownLayout", "unexpected error: {:?}", err);
    }

    #[test]
    fn required_layout_rejects_missing_declaration() {
        let err = compile_and_link_with_required_layout("text(\"a\")", &EmptyProvider).unwrap_err();
        assert_eq!(err.code, "LayoutRequired");
        assert_eq!(err.span.line, 1);
    }

    #[test]
    fn required_layout_rejects_later_declaration() {
        let entry = "layout(\"win_en-US\")\ntext(\"a\")\nlayout(\"win_en-US\")";
        let err = compile_and_link_with_required_layout(entry, &EmptyProvider).unwrap_err();
        assert_eq!(err.code, "LayoutMustBeFirst");
    }

    #[test]
    fn called_script_inherits_entry_layout() {
        let provider = LookupProvider {
            entries: vec![("sub", "layout(\"win_en-US\")\ntext(\"from sub\")")],
        };
        let entry = "layout(\"win_en-US\")\ncall sub";
        let program = compile_and_link_with_required_layout(entry, &provider).unwrap();
        assert_eq!(
            program
                .ops
                .iter()
                .filter(|op| matches!(op, OpOwned::Layout(_)))
                .count(),
            1
        );
    }
}
