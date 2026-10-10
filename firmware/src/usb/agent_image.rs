//! Internal artifact storage for CDC installation; never exposed as a USB drive.
// Keep the existing section and flash reservation to preserve artifact offsets
// and the persistent configuration layout.
const IMAGE_BYTES: usize = 4 * 1024 * 1024;

#[unsafe(link_section = ".msc_image")]
#[used]
static AGENT_IMAGE: [u8; IMAGE_BYTES] =
    *include_bytes!(concat!(env!("OUT_DIR"), "/host-agent.img"));

pub(super) fn image() -> &'static [u8] {
    &AGENT_IMAGE
}
