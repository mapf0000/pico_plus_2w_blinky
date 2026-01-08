use dsl_core::{compile_dsl_with_diag, MAX_DSL_LINES};

fn always_exists(_: &str) -> bool {
    true
}

fn never_exists(_: &str) -> bool {
    false
}

#[test]
fn too_many_lines_errors() {
    let mut dsl = String::new();
    for _ in 0..=MAX_DSL_LINES {
        dsl.push_str("tap A\n");
    }

    let err = compile_dsl_with_diag(&dsl, always_exists).expect_err("expected error");
    assert_eq!(err.code, "TooManyLines");
    assert_eq!(err.span.line as usize, MAX_DSL_LINES + 1);
}

#[test]
fn call_unknown_script_errors() {
    let dsl = "call missing";
    let err = compile_dsl_with_diag(dsl, never_exists).expect_err("expected error");
    assert_eq!(err.code, "UnknownScript");
    assert_eq!(err.span.line, 1);
}
