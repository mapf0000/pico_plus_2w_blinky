//! Build the host-agent FAT image embedded by the firmware.

use super::*;
use std::fmt::Write as _;

const IMAGE_BYTES: usize = 8 * 1024 * 1024;
const BYTES_PER_SECTOR: usize = 512;
const SECTORS_PER_CLUSTER: usize = 1;
const CLUSTER_SIZE: usize = BYTES_PER_SECTOR * SECTORS_PER_CLUSTER;
const RESERVED_SECTORS: usize = 1;
const NUM_FATS: usize = 2;
const ROOT_ENTRIES: usize = 512;
const MEDIA_DESCRIPTOR: u8 = 0xF8;
const VOLUME_LABEL_DEFAULT: &str = "PICO_AGENT";

const ROOT_DIR_SECTORS: usize = (ROOT_ENTRIES * 32).div_ceil(BYTES_PER_SECTOR);

const HOST_AGENT_NAME: &str = "HOSTAGNT";
const README_NAME: &str = "README";
const MISSING_NAME: &str = "MISSING";
const TXT_EXT: &str = "TXT";

struct TargetSpec {
    label: &'static str,
    required: bool,
    artifact_dir: &'static str,
    artifact_file: &'static str,
    volume_dir: &'static str,
    volume_file_name: &'static str,
    volume_file_ext: &'static str,
}

const TARGETS: &[TargetSpec] = &[
    TargetSpec {
        label: "macOS (aarch64)",
        required: true,
        artifact_dir: "aarch64-apple-darwin",
        artifact_file: "host-agent",
        volume_dir: "MAC",
        volume_file_name: HOST_AGENT_NAME,
        volume_file_ext: "",
    },
    TargetSpec {
        label: "Windows (x86_64)",
        required: false,
        artifact_dir: "x86_64-pc-windows-msvc",
        artifact_file: "host-agent.exe",
        volume_dir: "WIN",
        volume_file_name: HOST_AGENT_NAME,
        volume_file_ext: "EXE",
    },
    TargetSpec {
        label: "Linux (x86_64)",
        required: false,
        artifact_dir: "x86_64-unknown-linux-gnu",
        artifact_file: "host-agent",
        volume_dir: "LINUX",
        volume_file_name: HOST_AGENT_NAME,
        volume_file_ext: "",
    },
];

#[derive(Clone)]
struct FileSpec {
    name: [u8; 11],
    data: Vec<u8>,
}

struct DirSpec {
    name: [u8; 11],
    files: Vec<FileSpec>,
}

struct TargetStatus {
    volume_dir: &'static str,
    file_name: &'static str,
    file_ext: &'static str,
    present: bool,
}

pub fn register_reruns(cfg: &Config) {
    let host_agent = cfg.repo_root.join("apps/host-agent");
    cargo::rerun_if_changed(host_agent.join("artifacts"));
    cargo::rerun_if_changed(host_agent.join("Cargo.toml"));
}

pub fn prepare(cfg: &Config) -> Result<()> {
    if !cfg.target.starts_with("thumb") {
        return Ok(());
    }

    let artifacts_dir = cfg.repo_root.join("apps/host-agent/artifacts");
    let label = std::env::var(env_consts::MSC_LABEL)
        .ok()
        .unwrap_or_else(|| VOLUME_LABEL_DEFAULT.to_string());
    let label_bytes = normalize_label(&label)?;

    let mut root_files = Vec::new();
    let mut dir_specs = Vec::new();
    let mut status = Vec::new();

    for spec in TARGETS {
        let dir_name = short_name(spec.volume_dir, "")?;
        let mut files = Vec::new();
        let artifact_path = artifacts_dir
            .join(spec.artifact_dir)
            .join(spec.artifact_file);
        match fs::read(&artifact_path) {
            Ok(data) => {
                let file_name = short_name(spec.volume_file_name, spec.volume_file_ext)?;
                files.push(FileSpec {
                    name: file_name,
                    data,
                });
                status.push(TargetStatus {
                    volume_dir: spec.volume_dir,
                    file_name: spec.volume_file_name,
                    file_ext: spec.volume_file_ext,
                    present: true,
                });
            }
            Err(_) => {
                if spec.required {
                    cargo::warn(format!(
                        "msc: missing artifact for {} at {}",
                        spec.label,
                        artifact_path.display()
                    ));
                }
                let missing_name = short_name(MISSING_NAME, TXT_EXT)?;
                let mut msg = String::new();
                let _ = writeln!(msg, "Binary not included for {}.", spec.label);
                files.push(FileSpec {
                    name: missing_name,
                    data: msg.into_bytes(),
                });
                status.push(TargetStatus {
                    volume_dir: spec.volume_dir,
                    file_name: spec.volume_file_name,
                    file_ext: spec.volume_file_ext,
                    present: false,
                });
            }
        }

        dir_specs.push(DirSpec {
            name: dir_name,
            files,
        });
    }

    let readme_name = short_name(README_NAME, TXT_EXT)?;
    let readme = build_readme(&status, &label);
    root_files.push(FileSpec {
        name: readme_name,
        data: readme.into_bytes(),
    });

    let image = build_image(&label_bytes, root_files, dir_specs)?;
    let out_path = cfg.out_dir.join("host-agent.img");
    fs::write(&out_path, image).context("write host-agent.img")?;
    Ok(())
}

fn build_readme(status: &[TargetStatus], label: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "PICO HOST AGENT USB IMAGE");
    let _ = writeln!(out, "Volume label: {}", label);
    let _ = writeln!(out, "This volume is read-only.");
    let _ = writeln!(out);
    let _ = writeln!(out, "Contents:");
    for target in status {
        if target.file_ext.is_empty() {
            let _ = writeln!(
                out,
                "- /{}/{}{}",
                target.volume_dir,
                target.file_name,
                if target.present { "" } else { " (missing)" }
            );
        } else {
            let _ = writeln!(
                out,
                "- /{}/{}.{}{}",
                target.volume_dir,
                target.file_name,
                target.file_ext,
                if target.present { "" } else { " (missing)" }
            );
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "macOS quick start:");
    let _ = writeln!(
        out,
        "1) Copy /MAC/HOSTAGNT to a writable directory as host-agent"
    );
    let _ = writeln!(out, "2) chmod +x host-agent");
    let _ = writeln!(out, "3) ./host-agent vid=<VID> pid=<PID>");
    let _ = writeln!(out);
    let _ = writeln!(out, "To add other platforms, drop binaries into:");
    let _ = writeln!(out, "apps/host-agent/artifacts/<target>/");
    out
}

fn normalize_label(input: &str) -> Result<[u8; 11]> {
    let mut out = [b' '; 11];
    for (idx, ch) in input.bytes().take(11).enumerate() {
        out[idx] = match ch {
            b'a'..=b'z' => ch.to_ascii_uppercase(),
            b'A'..=b'Z' | b'0'..=b'9' | b' ' | b'_' => ch,
            _ => b'_',
        };
    }
    Ok(out)
}

fn short_name(name: &str, ext: &str) -> Result<[u8; 11]> {
    if name.is_empty() || name.len() > 8 || ext.len() > 3 {
        bail!("invalid short name: {name}.{ext}");
    }
    let mut out = [b' '; 11];
    for (idx, ch) in name.bytes().enumerate() {
        out[idx] = normalize_short_char(ch);
    }
    for (idx, ch) in ext.bytes().enumerate() {
        out[8 + idx] = normalize_short_char(ch);
    }
    Ok(out)
}

fn normalize_short_char(ch: u8) -> u8 {
    match ch {
        b'a'..=b'z' => ch.to_ascii_uppercase(),
        b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-' => ch,
        _ => b'_',
    }
}

struct Layout {
    total_sectors: usize,
    sectors_per_fat: usize,
    data_start_sector: usize,
    cluster_count: usize,
}

fn compute_layout() -> Result<Layout> {
    if !IMAGE_BYTES.is_multiple_of(BYTES_PER_SECTOR) {
        bail!("MSC image size must be sector-aligned");
    }
    let total_sectors = IMAGE_BYTES / BYTES_PER_SECTOR;
    let mut sectors_per_fat = 1usize;
    let cluster_count;
    loop {
        let data_sectors = total_sectors
            .saturating_sub(RESERVED_SECTORS + ROOT_DIR_SECTORS + NUM_FATS * sectors_per_fat);
        let clusters = data_sectors / SECTORS_PER_CLUSTER;
        let fat_bytes = (clusters + 2) * 2;
        let new_spf = fat_bytes.div_ceil(BYTES_PER_SECTOR);
        if new_spf == sectors_per_fat {
            cluster_count = clusters;
            break;
        }
        sectors_per_fat = new_spf;
    }

    if !(4085..=65524).contains(&cluster_count) {
        bail!("MSC image cluster count out of FAT16 range");
    }

    let data_start_sector = RESERVED_SECTORS + NUM_FATS * sectors_per_fat + ROOT_DIR_SECTORS;

    Ok(Layout {
        total_sectors,
        sectors_per_fat,
        data_start_sector,
        cluster_count,
    })
}

fn build_image(label: &[u8; 11], root_files: Vec<FileSpec>, dirs: Vec<DirSpec>) -> Result<Vec<u8>> {
    let layout = compute_layout()?;
    let mut image = vec![0u8; IMAGE_BYTES];

    write_boot_sector(&mut image[..BYTES_PER_SECTOR], label, &layout);

    let mut allocator = Allocator::new(layout.cluster_count);

    let mut root_allocs = Vec::new();
    for file in root_files {
        let (cluster, size) = allocator.alloc_file(&file.data)?;
        root_allocs.push(FileAlloc {
            name: file.name,
            attr: 0x01,
            cluster,
            size,
            data: file.data,
        });
    }

    let mut dir_allocs = Vec::new();
    for dir in dirs {
        let dir_cluster = allocator.alloc_dir()?;
        let mut files = Vec::new();
        for file in dir.files {
            let (cluster, size) = allocator.alloc_file(&file.data)?;
            files.push(FileAlloc {
                name: file.name,
                attr: 0x01,
                cluster,
                size,
                data: file.data,
            });
        }
        dir_allocs.push(DirAlloc {
            name: dir.name,
            cluster: dir_cluster,
            files,
        });
    }

    let fat0_start = RESERVED_SECTORS * BYTES_PER_SECTOR;
    let fat_size_bytes = layout.sectors_per_fat * BYTES_PER_SECTOR;
    write_fat(
        &mut image[fat0_start..fat0_start + fat_size_bytes],
        &allocator.fat,
    );
    let fat1_start = fat0_start + fat_size_bytes;
    write_fat(
        &mut image[fat1_start..fat1_start + fat_size_bytes],
        &allocator.fat,
    );

    let root_dir_start = (RESERVED_SECTORS + NUM_FATS * layout.sectors_per_fat) * BYTES_PER_SECTOR;
    let root_dir_bytes = ROOT_DIR_SECTORS * BYTES_PER_SECTOR;
    let mut root_entries = Vec::new();
    root_entries.push(DirEntry::volume_label(*label));
    for dir in &dir_allocs {
        root_entries.push(DirEntry::directory(dir.name, dir.cluster));
    }
    for file in &root_allocs {
        root_entries.push(DirEntry::file(
            file.name,
            file.attr,
            file.cluster,
            file.size,
        ));
    }
    write_dir_entries(
        &mut image[root_dir_start..root_dir_start + root_dir_bytes],
        &root_entries,
    )?;

    let data_start = layout.data_start_sector * BYTES_PER_SECTOR;
    for file in &root_allocs {
        write_file_data(&mut image, data_start, file.cluster, &file.data)?;
    }
    for dir in &dir_allocs {
        let cluster_offset = cluster_offset(data_start, dir.cluster)?;
        let mut entries = Vec::new();
        entries.push(DirEntry::dot(dir.cluster));
        entries.push(DirEntry::dotdot(0));
        for file in &dir.files {
            entries.push(DirEntry::file(
                file.name,
                file.attr,
                file.cluster,
                file.size,
            ));
            write_file_data(&mut image, data_start, file.cluster, &file.data)?;
        }
        write_dir_entries(
            &mut image[cluster_offset..cluster_offset + CLUSTER_SIZE],
            &entries,
        )?;
    }

    Ok(image)
}

struct Allocator {
    next_cluster: u16,
    cluster_count: usize,
    fat: Vec<u16>,
}

impl Allocator {
    fn new(cluster_count: usize) -> Self {
        let mut fat = vec![0u16; cluster_count + 2];
        fat[0] = 0xFFF8;
        fat[1] = 0xFFFF;
        Self {
            next_cluster: 2,
            cluster_count,
            fat,
        }
    }

    fn alloc_dir(&mut self) -> Result<u16> {
        self.alloc_clusters(1)
    }

    fn alloc_file(&mut self, data: &[u8]) -> Result<(u16, u32)> {
        let size = data.len() as u32;
        if size == 0 {
            return Ok((0, 0));
        }
        let clusters = data.len().div_ceil(CLUSTER_SIZE);
        let cluster = self.alloc_clusters(clusters)?;
        Ok((cluster, size))
    }

    fn alloc_clusters(&mut self, clusters: usize) -> Result<u16> {
        let start = self.next_cluster as usize;
        let end = start + clusters - 1;
        let max_cluster = self.cluster_count + 1;
        if end > max_cluster {
            bail!("MSC image too small for files");
        }

        let start_cluster = self.next_cluster;
        for offset in 0..clusters {
            let cur = start_cluster + offset as u16;
            let next = if offset + 1 == clusters {
                0xFFFF
            } else {
                cur + 1
            };
            self.fat[cur as usize] = next;
        }
        self.next_cluster = start_cluster + clusters as u16;
        Ok(start_cluster)
    }
}

struct FileAlloc {
    name: [u8; 11],
    attr: u8,
    cluster: u16,
    size: u32,
    data: Vec<u8>,
}

struct DirAlloc {
    name: [u8; 11],
    cluster: u16,
    files: Vec<FileAlloc>,
}

#[derive(Clone, Copy)]
struct DirEntry {
    name: [u8; 11],
    attr: u8,
    cluster: u16,
    size: u32,
}

impl DirEntry {
    fn volume_label(label: [u8; 11]) -> Self {
        Self {
            name: label,
            attr: 0x08,
            cluster: 0,
            size: 0,
        }
    }

    fn directory(name: [u8; 11], cluster: u16) -> Self {
        Self {
            name,
            attr: 0x10,
            cluster,
            size: 0,
        }
    }

    fn file(name: [u8; 11], attr: u8, cluster: u16, size: u32) -> Self {
        Self {
            name,
            attr,
            cluster,
            size,
        }
    }

    fn dot(cluster: u16) -> Self {
        let mut name = [b' '; 11];
        name[0] = b'.';
        Self::directory(name, cluster)
    }

    fn dotdot(cluster: u16) -> Self {
        let mut name = [b' '; 11];
        name[0] = b'.';
        name[1] = b'.';
        Self::directory(name, cluster)
    }
}

fn write_boot_sector(buf: &mut [u8], label: &[u8; 11], layout: &Layout) {
    buf.fill(0);
    buf[0] = 0xEB;
    buf[1] = 0x3C;
    buf[2] = 0x90;
    buf[3..11].copy_from_slice(b"MSDOS5.0");
    buf[11..13].copy_from_slice(&(BYTES_PER_SECTOR as u16).to_le_bytes());
    buf[13] = SECTORS_PER_CLUSTER as u8;
    buf[14..16].copy_from_slice(&(RESERVED_SECTORS as u16).to_le_bytes());
    buf[16] = NUM_FATS as u8;
    buf[17..19].copy_from_slice(&(ROOT_ENTRIES as u16).to_le_bytes());
    buf[19..21].copy_from_slice(&(layout.total_sectors as u16).to_le_bytes());
    buf[21] = MEDIA_DESCRIPTOR;
    buf[22..24].copy_from_slice(&(layout.sectors_per_fat as u16).to_le_bytes());
    buf[24..26].copy_from_slice(&63u16.to_le_bytes());
    buf[26..28].copy_from_slice(&255u16.to_le_bytes());
    buf[28..32].copy_from_slice(&0u32.to_le_bytes());
    buf[32..36].copy_from_slice(&0u32.to_le_bytes());
    buf[36] = 0x80;
    buf[37] = 0;
    buf[38] = 0x29;
    buf[39..43].copy_from_slice(&0x12345678u32.to_le_bytes());
    buf[43..54].copy_from_slice(label);
    buf[54..62].copy_from_slice(b"FAT16   ");
    buf[510] = 0x55;
    buf[511] = 0xAA;
}

fn write_fat(buf: &mut [u8], fat: &[u16]) {
    buf.fill(0);
    for (idx, entry) in fat.iter().enumerate() {
        let offset = idx * 2;
        if offset + 1 >= buf.len() {
            break;
        }
        buf[offset] = (*entry & 0xFF) as u8;
        buf[offset + 1] = (entry >> 8) as u8;
    }
}

fn write_dir_entries(buf: &mut [u8], entries: &[DirEntry]) -> Result<()> {
    let mut offset = 0usize;
    for entry in entries {
        if offset + 32 > buf.len() {
            bail!("directory entry overflow");
        }
        write_dir_entry(&mut buf[offset..offset + 32], entry);
        offset += 32;
    }
    Ok(())
}

fn write_dir_entry(buf: &mut [u8], entry: &DirEntry) {
    buf.fill(0);
    buf[..11].copy_from_slice(&entry.name);
    buf[11] = entry.attr;
    buf[26..28].copy_from_slice(&entry.cluster.to_le_bytes());
    buf[28..32].copy_from_slice(&entry.size.to_le_bytes());
}

fn write_file_data(image: &mut [u8], data_start: usize, cluster: u16, data: &[u8]) -> Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    let mut offset = cluster_offset(data_start, cluster)?;
    for chunk in data.chunks(CLUSTER_SIZE) {
        let end = offset + chunk.len();
        if end > image.len() {
            bail!("MSC image overflow");
        }
        image[offset..end].copy_from_slice(chunk);
        offset += CLUSTER_SIZE;
    }
    Ok(())
}

fn cluster_offset(data_start: usize, cluster: u16) -> Result<usize> {
    if cluster < 2 {
        bail!("invalid cluster {cluster}");
    }
    let idx = (cluster as usize - 2) * CLUSTER_SIZE;
    Ok(data_start + idx)
}
