//! Typed built-in keyboard presets compiled on the host, never interpreted on-device.
use super::*;
use keyboard_core::{KeyTap, LayoutId, OpOwned, ProgramOwned, bytecode};

struct Preset {
    name: &'static str,
    layout: LayoutId,
    launches_agent: bool,
    available: bool,
    program: ProgramOwned,
    action: &'static str,
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
fn cdc_installer(available: bool, layout: LayoutId) -> Result<Preset> {
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
            text(super::bootstrap::COMMAND),
            chord("ENTER")?,
        ],
    };
    Ok(Preset {
        action: "CdcInstall",
        name: match layout {
            LayoutId::Us => "macOS: Install via CDC (US)",
            _ => "macOS: Install via CDC (DE)",
        },
        layout,
        launches_agent: true,
        available,
        program,
    })
}
fn catalog(agent_present: bool) -> Result<Vec<Preset>> {
    let mut presets = Vec::new();
    presets.push(Preset {
        action: "Keyboard",
        name: "Keyboard test",
        layout: LayoutId::Us,
        launches_agent: false,
        available: true,
        program: ProgramOwned {
            ops: vec![OpOwned::Layout(LayoutId::Us), text("Hello from Pico!")],
        },
    });
    let enabled = agent_present && std::env::var(super::env_consts::USB_SERIAL).is_ok();
    presets.push(Preset {
        name: "macOS: Arm manual CDC install",
        layout: LayoutId::Us,
        launches_agent: true,
        available: enabled,
        action: "CdcArm",
        program: ProgramOwned {
            ops: vec![OpOwned::Layout(LayoutId::Us), OpOwned::DelayMs(0)],
        },
    });
    for layout in [LayoutId::MacDeDe, LayoutId::Us] {
        presets.push(cdc_installer(enabled, layout)?);
    }
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

fn validate_catalog(presets: &[Preset]) -> Result<()> {
    if presets.is_empty() {
        bail!("keyboard preset catalog must not be empty");
    }
    // Metadata remains in flash. Bound catalog growth and keep labels within the
    // display's 96-character formatting limit; viewport ellipsis is applied later.
    if presets.len() > 64 {
        bail!("keyboard preset catalog exceeds 64 entries");
    }
    for preset in presets {
        if preset.name.is_empty()
            || preset.name.len() > 96
            || !preset.name.is_ascii()
            || preset.name.chars().any(char::is_control)
        {
            bail!("keyboard preset name must be 1..=96 printable ASCII characters");
        }
    }
    Ok(())
}
pub(super) fn prepare(cfg: &Config, agent_present: bool) -> Result<()> {
    use std::fmt::Write;
    let mut generated = String::from(
        "// Generated from typed Rust presets; do not edit.\nstatic PRESETS: &[Preset] = &[\n",
    );
    let presets = catalog(agent_present)?;
    validate_catalog(&presets)?;
    for preset in presets {
        let bytes = compile(&preset)?;
        writeln!(
            generated,
            "Preset {{ name: {:?}, layout: {:?}, launches_agent: {}, available: {}, program: &{:?}, action: PresetAction::{} }},",
            preset.name,
            preset.layout.name(),
            preset.launches_agent,
            preset.available,
            bytes,
            preset.action,
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
        for preset in catalog(true).unwrap() {
            let bytes = compile(&preset).unwrap();
            let decoded = bytecode::decode_to_flat(&bytes).unwrap();
            assert_eq!(bytecode::encode(&decoded).unwrap(), bytes);
        }
    }
    #[test]
    fn catalog_uses_only_cdc_for_installation_and_disables_missing_agents() {
        let presets = catalog(false).unwrap();
        assert_eq!(presets.len(), 4);
        for preset in presets {
            assert_eq!(preset.available, !preset.launches_agent);
            assert!(!preset.launches_agent || preset.action != "Keyboard");
            assert!(
                !preset
                    .program
                    .ops
                    .iter()
                    .any(|op| matches!(op, OpOwned::Text { s, .. } if s.contains("/Volumes/")))
            );
            if preset.action == "CdcInstall" {
                assert!(preset.program.ops.iter().any(
                    |op| matches!(op, OpOwned::Text { s, .. } if s == super::super::bootstrap::COMMAND)
                ));
            }
        }
    }

    #[test]
    fn catalog_bounds_reject_empty_oversized_and_unrenderable_metadata() {
        assert!(validate_catalog(&[]).is_err());
        let mut presets = catalog(true).unwrap();
        assert!(validate_catalog(&presets).is_ok());
        presets[0].name = "invalid\nname";
        assert!(validate_catalog(&presets).is_err());
        presets[0].name = "valid name";
        while presets.len() <= 64 {
            presets.extend(catalog(true).unwrap());
        }
        assert!(validate_catalog(&presets).is_err());
    }
}
