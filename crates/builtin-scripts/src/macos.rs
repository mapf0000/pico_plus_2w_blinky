use super::BuiltinScript;

const OPEN_TERMINAL_DSL: &str = concat!(
    "layout(\"win_en-US\")\n",
    "modtap(\"LGUI+SPACE\")\n",
    "delay(100)\n",
    "text(\"Terminal\", 5)\n",
    "delay(100)\n",
    "tap(\"ENTER\")\n",
    "delay(1000)\n",
    "modtap(\"LGUI+N\")\n",
);

const MACOS_HOST_AGENT_DSL: &str = concat!(
    "layout(\"mac_de-DE\")\n",
    "call open_terminal\n",
    "delay(1200)\n",
    "text(\"mkdir -p ~/pico-agent && cp /Volumes/PICO_AGENT/MAC/HOSTAGNT ~/pico-agent/ && chmod +x ~/pico-agent/HOSTAGNT && nohup ~/pico-agent/HOSTAGNT vid=0x1209 pid=0x0001 >/dev/null 2>&1 </dev/null & disown; exit\", 1)\n",
    "tap(\"ENTER\")\n",
    "delay(5000)\n",
    "modtap(\"LGUI+W\")\n",
);

const MACOS_HOST_AGENT_DEBUG_DSL: &str = concat!(
    "layout(\"mac_de-DE\")\n",
    "call open_terminal\n",
    "delay(1200)\n",
    "text(\"mkdir -p ~/pico-agent && cp /Volumes/PICO_AGENT/MAC/HOSTAGNT ~/pico-agent/ && chmod +x ~/pico-agent/HOSTAGNT && ~/pico-agent/HOSTAGNT vid=0x1209 pid=0x0001\")\n",
    "tap(\"ENTER\")\n",
);

const MAC_ASSISTANT_US_DSL: &str = concat!(
    "layout(\"win_en-US\")\n",
    "delay(1200)\n",
    "tap(\"ENTER\")\n",
    "delay(800)\n",
    "tap(\"Z\")\n",
    "delay(800)\n",
    "tap(\"SLASH\")\n",
    "delay(800)\n",
    "tap(\"ENTER\")\n",
    "delay(600)\n",
);

const MAC_ASSISTANT_DE_DSL: &str = concat!(
    "layout(\"mac_de-DE\")\n",
    "delay(1200)\n",
    "tap(\"ENTER\")\n",
    "delay(800)\n",
    "text(\"<\")\n",
    "delay(800)\n",
    "text(\"-\")\n",
    "delay(800)\n",
    "tap(\"ENTER\")\n",
    "delay(600)\n",
);

const DEMO_CALL_DSL: &str = concat!(
    "layout(\"win_en-US\")\n",
    "call hello_world\n",
    "delay(500)\n",
    "call open_terminal\n",
);

pub const OPEN_TERMINAL: BuiltinScript = BuiltinScript {
    id: "open_terminal",
    name: "Open macOS Terminal",
    description: "Spotlight -> type 'Terminal' -> Enter -> New Window",
    dsl: OPEN_TERMINAL_DSL,
};

pub const MACOS_HOST_AGENT: BuiltinScript = BuiltinScript {
    id: "macos_host_agent",
    name: "macOS: Launch host-agent (local copy)",
    description: "Copies HOSTAGNT to ~/pico-agent and runs it with VID/PID (assumes German input source on macOS)",
    dsl: MACOS_HOST_AGENT_DSL,
};

pub const MACOS_HOST_AGENT_DEBUG: BuiltinScript = BuiltinScript {
    id: "macos_host_agent_debug",
    name: "macOS: Launch host-agent (debug)",
    description: "Foreground run with visible output (assumes German input source on macOS)",
    dsl: MACOS_HOST_AGENT_DEBUG_DSL,
};

pub const ASSISTANT_US: BuiltinScript = BuiltinScript {
    id: "assistant_us",
    name: "macOS Keyboard Assistant (US)",
    description: "Guide the Keyboard Setup Assistant for US ANSI (Z then / keys)",
    dsl: MAC_ASSISTANT_US_DSL,
};

pub const ASSISTANT_DE: BuiltinScript = BuiltinScript {
    id: "assistant_de",
    name: "macOS Keyboard Assistant (DE)",
    description: "Guide the Keyboard Setup Assistant for German ISO (< > | then - _ keys)",
    dsl: MAC_ASSISTANT_DE_DSL,
};

pub const DEMO_CALL: BuiltinScript = BuiltinScript {
    id: "demo_call",
    name: "Demo: Call Hello + Terminal",
    description: "Demonstrates calling other scripts by id",
    dsl: DEMO_CALL_DSL,
};
