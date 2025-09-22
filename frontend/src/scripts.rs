pub struct BuiltinScript {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub dsl: &'static str,
}

const OPEN_TERMINAL_DSL: &str = concat!(
    "modtap(\"LGUI+SPACE\")\n",
    "delay(400)\n",
    "text(\"Terminal\", 10)\n",
    "delay(200)\n",
    "tap(\"ENTER\")\n",
    "delay(1500)\n",
    "modtap(\"LGUI+N\")\n",
);

const MAC_ASSISTANT_ONCE_DSL: &str = concat!(
    "delay(1200)\n",
    "tap(\"ENTER\")\n",
    "delay(800)\n",
    "tap(\"Z\")\n",
    "delay(800)\n",
    "tap(\"SLASH\")\n",
    "delay(800)\n",
    "tap(\"ENTER\")\n",
);

const HELLO_WORLD_DSL: &str = "text(\"Hello, world\", 20)\n";

const DEMO_CALL_DSL: &str = concat!("call hello_world\n", "delay(500)\n", "call open_terminal\n",);

const BUILTIN_SCRIPTS: &[BuiltinScript] = &[
    BuiltinScript {
        id: "open_terminal",
        name: "Open macOS Terminal",
        description: "Spotlight → type 'Terminal' → Enter → New Window",
        dsl: OPEN_TERMINAL_DSL,
    },
    BuiltinScript {
        id: "assistant_once",
        name: "macOS Keyboard Assistant (once)",
        description: "Guide the Keyboard Setup Assistant with ANSI hints",
        dsl: MAC_ASSISTANT_ONCE_DSL,
    },
    BuiltinScript {
        id: "hello_world",
        name: "Hello, world (slow)",
        description: "Type 'Hello, world' with a small delay",
        dsl: HELLO_WORLD_DSL,
    },
    BuiltinScript {
        id: "demo_call",
        name: "Demo: Call Hello + Terminal",
        description: "Demonstrates calling other scripts by id",
        dsl: DEMO_CALL_DSL,
    },
];

pub fn all() -> &'static [BuiltinScript] {
    BUILTIN_SCRIPTS
}

pub fn lookup(id: &str) -> Option<&'static str> {
    BUILTIN_SCRIPTS.iter().find(|s| s.id == id).map(|s| s.dsl)
}
