//! Typed built-in keyboard presets compiled on the host, never interpreted on-device.
use super::*;
use keyboard_core::{KeyTap, LayoutId, OpOwned, ProgramOwned, bytecode};

pub(super) struct AgentImage<'a> {
    pub volume: &'a str,
    pub directory: &'a str,
    pub binary: &'a str,
    pub present: bool,
}
struct Preset {
    name: &'static str,
    layout: LayoutId,
    launches_agent: bool,
    available: bool,
    program: ProgramOwned,
}
fn chord(value: &str) -> Result<OpOwned> {
    let (mods, usage) =
        keyboard_core::parse_modtap(value).map_err(|_| anyhow::anyhow!("invalid preset chord"))?;
    Ok(OpOwned::Tap(KeyTap { usage, mods }))
}
fn text(value: impl Into<String>) -> OpOwned {
    OpOwned::Text {
        s: value.into(),
        delay_ms: 1,
    }
}
fn launcher(image: &AgentImage<'_>, debug: bool, layout: LayoutId) -> Result<Preset> {
    // FAT label/path normalization permits no quotes. Quote the entire source path
    // so volume labels containing spaces keep working.
    let source = format!(
        "/Volumes/{}/{}/{}",
        image.volume, image.directory, image.binary
    );
    let command = if debug {
        format!(
            "mkdir -p ~/pico-agent && cp '{source}' ~/pico-agent/HOSTAGNT && chmod +x ~/pico-agent/HOSTAGNT && ~/pico-agent/HOSTAGNT"
        )
    } else {
        format!(
            "mkdir -p ~/pico-agent && cp '{source}' ~/pico-agent/HOSTAGNT && chmod +x ~/pico-agent/HOSTAGNT && {{ nohup ~/pico-agent/HOSTAGNT >/dev/null 2>&1 </dev/null & disown; }}"
        )
    };
    let program = ProgramOwned {
        ops: vec![
            OpOwned::Layout(layout),
            chord("LGUI+SPACE")?,
            OpOwned::DelayMs(300),
            text("Terminal"),
            OpOwned::DelayMs(300),
            chord("ENTER")?,
            OpOwned::DelayMs(1500),
            chord("LGUI+N")?,
            OpOwned::DelayMs(1200),
            text(command),
            chord("ENTER")?,
        ],
    };
    Ok(Preset {
        name: match (layout, debug) {
            (LayoutId::Us, false) => "macOS: Launch agent (US)",
            (LayoutId::Us, true) => "macOS: Debug agent (US)",
            (_, false) => "macOS: Launch agent (DE)",
            (_, true) => "macOS: Debug agent (DE)",
        },
        layout,
        launches_agent: true,
        available: image.present,
        program,
    })
}
fn catalog(image: &AgentImage<'_>) -> Result<Vec<Preset>> {
    let mut presets = Vec::new();
    for layout in [LayoutId::MacDeDe, LayoutId::Us] {
        for debug in [false, true] {
            presets.push(launcher(image, debug, layout)?);
        }
    }
    presets.push(Preset {
        name: "Keyboard test",
        layout: LayoutId::Us,
        launches_agent: false,
        available: true,
        program: ProgramOwned {
            ops: vec![OpOwned::Layout(LayoutId::Us), text("Hello from Pico!")],
        },
    });
    Ok(presets)
}
fn compile(preset: &Preset) -> Result<Vec<u8>> {
    let flat = keyboard_core::lower_to_flat(&preset.program)
        .map_err(|e| anyhow::anyhow!("preset lowering failed: {}", e.message))?;
    let bytes =
        bytecode::encode(&flat).map_err(|e| anyhow::anyhow!("preset encoding failed: {e:?}"))?;
    bytecode::validate(&bytes).map_err(|e| anyhow::anyhow!("preset exceeds KBD1 limits: {e:?}"))?;
    Ok(bytes)
}
pub(super) fn prepare(cfg: &Config, image: AgentImage<'_>) -> Result<()> {
    use std::fmt::Write;
    let mut generated = String::from(
        "// Generated from typed Rust presets; do not edit.\nstatic PRESETS: &[Preset] = &[\n",
    );
    for preset in catalog(&image)? {
        let bytes = compile(&preset)?;
        writeln!(
            generated,
            "Preset {{ name: {:?}, layout: {:?}, launches_agent: {}, available: {}, program: &{:?} }},",
            preset.name,
            preset.layout.name(),
            preset.launches_agent,
            preset.available,
            bytes
        )?;
    }
    generated.push_str("];\n");
    fs::write(cfg.out_dir.join("keyboard_presets.rs"), generated).context("write keyboard presets")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_preset_fits_and_round_trips_through_the_strict_keyboard_decoder() {
        let image = AgentImage {
            volume: "CUSTOM DISK",
            directory: "MAC",
            binary: "HOSTAGNT",
            present: true,
        };
        for preset in catalog(&image).unwrap() {
            let bytes = compile(&preset).unwrap();
            let decoded = bytecode::decode_to_flat(&bytes).unwrap();
            assert_eq!(bytecode::encode(&decoded).unwrap(), bytes);
        }
    }
    #[test]
    fn launchers_follow_image_metadata_and_missing_binaries_disable_only_launchers() {
        let image = AgentImage {
            volume: "MY DISK",
            directory: "OTHER",
            binary: "AGENT",
            present: false,
        };
        for preset in catalog(&image).unwrap() {
            if preset.launches_agent {
                assert!(!preset.available);
                assert!(preset.program.ops.iter().any(|op| matches!(op, OpOwned::Text { s, .. } if s.contains("'/Volumes/MY DISK/OTHER/AGENT'"))));
            } else {
                assert!(preset.available);
            }
        }
    }
}
