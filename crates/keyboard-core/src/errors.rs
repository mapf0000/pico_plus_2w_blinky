//! Keyboard lowering diagnostics.

#[cfg(feature = "std")]
use std::string::String;

#[cfg(not(feature = "std"))]
use alloc::string::String;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub line: u16,
    pub col: u16,
    pub len: u16,
}

impl Span {
    pub fn new(line: u16, col: u16, len: u16) -> Self {
        Self { line, col, len }
    }

    pub fn line(line: u16) -> Self {
        Self {
            line,
            col: 1,
            len: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
    pub span: Span,
    pub suggestion: Option<String>,
}

impl CompileError {
    pub fn error(code: &'static str, message: impl Into<String>, span: Span) -> Self {
        Self {
            severity: Severity::Error,
            code,
            message: message.into(),
            span,
            suggestion: None,
        }
    }

    pub fn warning(
        code: &'static str,
        message: impl Into<String>,
        span: Span,
        suggestion: Option<String>,
    ) -> Self {
        Self {
            severity: Severity::Warning,
            code,
            message: message.into(),
            span,
            suggestion,
        }
    }
}

pub fn message_for_code(code: &'static str) -> &'static str {
    match code {
        "TooManyLines" => "Program exceeds maximum length",
        "UnknownCommand" => "Unknown command",
        "InvalidLine" => "Invalid syntax",
        "ParseKey" => "Unrecognized key name",
        "ParseMod" => "Unrecognized modifier",
        "ParseDelay" => "Invalid delay value",
        "TextEmpty" => "text() requires a non-empty string",
        "UnknownLayout" => "Unknown layout id",
        "LayoutNotEnabled" => "Layout not enabled at build time",
        "LayoutRequired" => "Script must begin with layout(\"...\")",
        "LayoutMustBeFirst" => "layout(\"...\") is only allowed as the first command",
        "UnknownScript" => "call refers to unknown script",
        "RecursionTooDeep" => "Recursive call detected",
        _ => code,
    }
}
