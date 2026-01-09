use dsl_core::{FlatProgram, bytecode, compile_and_link, lower_to_flat_us};

fn empty_provider<'a>(_: &'a str) -> Option<&'a str> {
    None
}

#[test]
fn compile_lower_encode_decode_roundtrip() {
    let entry = "tap(\"A\")\ndelay(10)\ntext(\"B\", 0)";
    let owned = compile_and_link(entry, &empty_provider).expect("compile_and_link");
    let flat = lower_to_flat_us(&owned).expect("lower_to_flat_us");

    let bytes = bytecode::encode(&flat).expect("encode");
    let decoded = bytecode::decode_to_flat(&bytes).expect("decode_to_flat");

    assert_eq!(decoded, flat);
}

#[test]
fn encode_empty_program() {
    let program = FlatProgram::new();
    let bytes = bytecode::encode(&program).expect("encode");
    let decoded = bytecode::decode_to_flat(&bytes).expect("decode_to_flat");
    assert_eq!(decoded, program);
}
