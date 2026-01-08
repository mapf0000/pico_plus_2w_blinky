#[cfg(feature = "std")]
use std::{borrow::ToOwned, string::String, vec::Vec};

#[cfg(not(feature = "std"))]
use alloc::{borrow::ToOwned, string::String, vec::Vec};

use crate::limits::MAX_TOTAL_FLAT_OPS;
use crate::parser::compile_dsl_with_diag;
use crate::preprocess::{map_pre_line, preprocess, PreprocessOptions};
use crate::{DslError, DslErrorAt, Op, OpOwned, Program, ProgramOwned};

/// Simple provider used during linking. Returns DSL text for a script id.
pub trait ScriptProvider {
    fn get<'a>(&self, id: &'a str) -> Option<&'a str>;
}

impl<F> ScriptProvider for F
where
    for<'a> F: Fn(&'a str) -> Option<&'a str>,
{
    fn get<'a>(&self, id: &'a str) -> Option<&'a str> {
        (self)(id)
    }
}

/// Compile `entry_dsl`, resolve & inline all `call`s using `provider`,
/// and return an **owned** program with no `Call` ops.
pub fn compile_and_link<'a>(
    entry_dsl: &'a str,
    provider: &impl ScriptProvider,
) -> Result<ProgramOwned, DslErrorAt> {
    // Preprocess entry script (repeat/let), then parse.
    let pre = preprocess(entry_dsl, &PreprocessOptions::default()).map_err(|e| DslErrorAt {
        kind: DslError::InvalidLine,
        line: e.line,
    })?;
    let exists = |id: &str| provider.get(id).is_some();
    let ast = match compile_dsl_with_diag(&pre.text, exists) {
        Ok(p) => p,
        Err(e) => {
            let line = map_pre_line(e.line, &pre.sourcemap);
            return Err(DslErrorAt { kind: e.kind, line });
        }
    };
    let mut out = ProgramOwned::new();
    let mut stack: Vec<String> = Vec::new();
    inline_into_owned(&ast, provider, &mut out, &mut stack)?;
    Ok(out)
}

fn inline_into_owned(
    ast: &Program<'_>,
    provider: &impl ScriptProvider,
    out: &mut ProgramOwned,
    stack: &mut Vec<String>,
) -> Result<(), DslErrorAt> {
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
            Op::Layout(layout) => out.ops.push(OpOwned::Layout(*layout)),
            Op::Call { id } => {
                // cycle detection
                if stack.iter().any(|s| s == id) {
                    return Err(DslErrorAt {
                        kind: DslError::RecursionTooDeep,
                        line: (idx as u16) + 1,
                    });
                }
                stack.push((*id).to_owned());
                let Some(text) = provider.get(id) else {
                    return Err(DslErrorAt {
                        kind: DslError::UnknownScript,
                        line: (idx as u16) + 1,
                    });
                };
                // Preprocess callee independently (file-local scope), then parse and inline.
                let pre =
                    preprocess(text, &PreprocessOptions::default()).map_err(|e| DslErrorAt {
                        kind: DslError::InvalidLine,
                        line: e.line,
                    })?;
                let exists = |sid: &str| provider.get(sid).is_some();
                let sub = match compile_dsl_with_diag(&pre.text, exists) {
                    Ok(p) => p,
                    Err(e) => {
                        let line = map_pre_line(e.line, &pre.sourcemap);
                        return Err(DslErrorAt { kind: e.kind, line });
                    }
                };
                inline_into_owned(&sub, provider, out, stack)?;
                stack.pop();
            }
        }
        if out.ops.len() > MAX_TOTAL_FLAT_OPS {
            // reusing the cap for owned too
            return Err(DslErrorAt {
                kind: DslError::TooManyLines,
                line: 0,
            });
        }
    }
    Ok(())
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use crate::{preprocess, DslError, PreprocessOptions};

    fn empty_provider<'a>(_: &'a str) -> Option<&'a str> {
        None
    }

    struct LookupProvider {
        entries: Vec<(&'static str, &'static str)>,
    }

    impl ScriptProvider for LookupProvider {
        fn get<'a>(&self, id: &'a str) -> Option<&'a str> {
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
        assert_eq!(err.line, 2);
        assert!(matches!(err.kind, DslError::UnknownCommand));
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
        assert_eq!(err.line, 1);
        assert!(matches!(err.kind, DslError::UnknownCommand));
    }

    #[test]
    fn unknown_layout_errors() {
        let entry = "layout(\"win_xx-YY\")\ntext(\"a\")";
        let _ = preprocess(entry, &PreprocessOptions::default())
            .unwrap_or_else(|e| panic!("preprocess: {:?}", e));
        let err = compile_and_link(entry, &empty_provider).unwrap_err();
        assert_eq!(err.line, 1);
        assert!(
            matches!(err.kind, DslError::UnknownLayout),
            "unexpected error: {:?}",
            err
        );
    }
}
