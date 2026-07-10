use dsl_core::bytecode;
use wasm_bindgen_test::*;

use frontend::{codec, dsl, scripts};

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn builtins_compile_and_roundtrip() {
    for script in scripts::all() {
        let bytecode = dsl::compile(script.dsl)
            .unwrap_or_else(|err| panic!("compile builtin {} failed: {}", script.id, err.message));
        assert!(
            !bytecode.is_empty(),
            "{} compiled to empty bytecode",
            script.id
        );

        let encoded = codec::encode_hex(&bytecode);
        assert!(!encoded.is_empty(), "{} encoded hex empty", script.id);

        let decoded = codec::decode_hex(&encoded).expect("hex decode");
        assert_eq!(decoded, bytecode, "{} hex roundtrip", script.id);

        let flat = bytecode::decode_to_flat(&decoded).expect("decode bytecode");
        assert!(
            !flat.ops.is_empty(),
            "{} produced no operations after decode",
            script.id
        );
    }
}

#[wasm_bindgen_test]
fn invalid_script_fails() {
    match dsl::compile("layout(\"win_en-US\")\ncall missing_script") {
        Ok(_) => panic!("expected compile failure"),
        Err(err) => assert!(
            err.message.to_ascii_lowercase().contains("unknown"),
            "unexpected error: {}",
            err.message
        ),
    }
}
