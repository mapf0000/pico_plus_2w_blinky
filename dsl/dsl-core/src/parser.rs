use core::str::FromStr;

use crate::keycodes::{parse_key, parse_modtap};
use crate::limits::{MAX_DSL_DELAY_MS, MAX_DSL_LINES};
use crate::{DslError, DslErrorAt, KeyTap, LayoutId, LayoutParseError, Op, Program};

pub trait ScriptExists {
    fn exists(&self, id: &str) -> bool;
}

impl<F> ScriptExists for F
where
    F: Fn(&str) -> bool,
{
    fn exists(&self, id: &str) -> bool {
        (self)(id)
    }
}

fn split_head(s: &str) -> Option<(&str, &str)> {
    let mut it = s.splitn(2, char::is_whitespace);
    let head = it.next()?;
    let tail = it.next().unwrap_or("");
    Some((head, tail))
}

fn eq_ci(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

pub fn compile_dsl_with_diag<'a>(
    dsl: &'a str,
    exists: impl ScriptExists,
) -> Result<Program<'a>, DslErrorAt> {
    let mut prog = Program::new();
    let mut counted_nonempty = 0usize;
    for (lineno0, raw) in dsl.lines().enumerate() {
        let line_no = (lineno0 + 1) as u16;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        counted_nonempty += 1;
        if counted_nonempty > MAX_DSL_LINES {
            return Err(DslErrorAt {
                kind: DslError::TooManyLines,
                line: line_no,
            });
        }

        let (cmd, rest) = split_head(line).ok_or(DslErrorAt {
            kind: DslError::InvalidLine,
            line: line_no,
        })?;
        if eq_ci(cmd, "tap") {
            let key_name = rest.trim();
            if key_name.is_empty() {
                return Err(DslErrorAt {
                    kind: DslError::InvalidLine,
                    line: line_no,
                });
            }
            let usage = parse_key(key_name).ok_or(DslErrorAt {
                kind: DslError::ParseKey,
                line: line_no,
            })?;
            prog.ops.push(Op::Tap(KeyTap { usage, mods: 0 }));
        } else if eq_ci(cmd, "modtap") {
            let arg = rest.trim();
            if arg.is_empty() {
                return Err(DslErrorAt {
                    kind: DslError::InvalidLine,
                    line: line_no,
                });
            }
            let (mods, usage) = parse_modtap(arg).map_err(|k| DslErrorAt {
                kind: k,
                line: line_no,
            })?;
            prog.ops.push(Op::Tap(KeyTap { usage, mods }));
        } else if eq_ci(cmd, "delay") {
            let ms: u64 = rest.trim().parse::<u64>().map_err(|_| DslErrorAt {
                kind: DslError::ParseDelay,
                line: line_no,
            })?;
            let ms = core::cmp::min(ms, MAX_DSL_DELAY_MS) as u32;
            if ms != 0 {
                prog.ops.push(Op::DelayMs(ms));
            }
        } else if eq_ci(cmd, "text") {
            let (text, delay_ms) = parse_text_args(rest).map_err(|k| DslErrorAt {
                kind: k,
                line: line_no,
            })?;
            let delay_ms = core::cmp::min(delay_ms as u64, MAX_DSL_DELAY_MS) as u16;
            if !text.is_empty() {
                prog.ops.push(Op::Text { s: text, delay_ms });
            }
        } else if eq_ci(cmd, "layout") {
            let raw = rest.trim();
            if raw.is_empty() {
                return Err(DslErrorAt {
                    kind: DslError::InvalidLine,
                    line: line_no,
                });
            }
            let id = if raw.starts_with('"') {
                if raw.ends_with('"') && raw.len() >= 2 {
                    &raw[1..raw.len() - 1]
                } else {
                    return Err(DslErrorAt {
                        kind: DslError::InvalidLine,
                        line: line_no,
                    });
                }
            } else {
                raw
            };
            let layout = match LayoutId::from_str(id) {
                Ok(layout) => layout,
                Err(LayoutParseError::NotEnabled) => {
                    return Err(DslErrorAt {
                        kind: DslError::LayoutNotEnabled,
                        line: line_no,
                    })
                }
                Err(LayoutParseError::Unknown) => {
                    return Err(DslErrorAt {
                        kind: DslError::UnknownLayout,
                        line: line_no,
                    })
                }
            };
            prog.ops.push(Op::Layout(layout));
        } else if eq_ci(cmd, "call") {
            let id = rest.trim();
            if id.is_empty() {
                return Err(DslErrorAt {
                    kind: DslError::InvalidLine,
                    line: line_no,
                });
            }
            if !exists.exists(id) {
                return Err(DslErrorAt {
                    kind: DslError::UnknownScript,
                    line: line_no,
                });
            }
            prog.ops.push(Op::Call { id });
        } else {
            return Err(DslErrorAt {
                kind: DslError::UnknownCommand,
                line: line_no,
            });
        }
    }
    Ok(prog)
}

fn parse_text_args(rest: &str) -> Result<(&str, u64), DslError> {
    let r = rest.trim();
    if r.is_empty() {
        return Err(DslError::TextEmpty);
    }
    let mut delay_ms: u64 = 10;
    if let Some(idx) = r.rfind(char::is_whitespace) {
        let (lhs, rhs) = r.split_at(idx);
        let maybe = rhs.trim();
        if !maybe.is_empty() {
            if let Ok(n) = maybe.parse::<u64>() {
                delay_ms = core::cmp::min(n, MAX_DSL_DELAY_MS);
                let text = lhs.trim_end();
                return Ok((text, delay_ms));
            }
        }
    }
    Ok((r, delay_ms))
}
