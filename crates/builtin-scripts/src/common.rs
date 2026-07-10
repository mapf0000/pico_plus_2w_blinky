use super::BuiltinScript;

const HELLO_WORLD_DSL: &str = concat!("layout(\"win_en-US\")\n", "text(\"Hello, world\", 20)\n",);

pub const HELLO_WORLD: BuiltinScript = BuiltinScript {
    id: "hello_world",
    name: "Hello, world (slow)",
    description: "Type 'Hello, world' with a small delay",
    dsl: HELLO_WORLD_DSL,
};
