// -------- Errors & Diagnostics --------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DslError {
    TooManyLines,
    UnknownCommand,
    InvalidLine,
    ParseKey,
    ParseMod,
    ParseDelay,
    TextEmpty,
    UnknownLayout,
    LayoutNotEnabled,
    UnknownScript,
    RecursionTooDeep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DslErrorAt {
    pub kind: DslError,
    /// 1-based line number in the current DSL being compiled.
    pub line: u16,
}
