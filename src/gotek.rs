use anyhow::{Context, Result, bail};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const FLOPPY_SIZE: usize = 1_474_560;
pub const SLOT_COUNT: u16 = 1000;
pub const MANAGED_IMAGE: &str = ".floppy.img";
pub const MANAGED_HASH: &str = ".floppy.hash";
pub const WRITTEN_HASH: &str = ".floppy.written.hash";
const SLOT_PADDING: usize = 98_304;
const SLOT_STRIDE: usize = FLOPPY_SIZE + SLOT_PADDING;

const BYTES_PER_SECTOR: usize = 512;
const SECTORS_PER_CLUSTER: u8 = 1;
const RESERVED_SECTORS: u16 = 1;
const FAT_COUNT: u8 = 2;
const ROOT_ENTRY_COUNT: u16 = 224;
const TOTAL_SECTORS: u16 = 2880;
const SECTORS_PER_FAT: u16 = 9;
const ROOT_DIR_SECTORS: usize = 14;
const FIRST_ROOT_SECTOR: usize = 19;
const FIRST_DATA_SECTOR: usize = 33;
const DATA_CLUSTER_COUNT: u16 = 2847;

#[derive(Debug, Clone)]
pub struct GotekOptions {
    pub root: PathBuf,
}

impl Default for GotekOptions {
    fn default() -> Self {
        Self { root: gotek_root() }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PackReport {
    pub root: PathBuf,
    pub slots: Vec<PackSlotReport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PackSlotReport {
    pub slot: u16,
    pub status: String,
    pub image: Option<PathBuf>,
    pub hash: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SlotIoReport {
    pub slot: u16,
    pub image: PathBuf,
    pub offset: u64,
    pub bytes: usize,
    pub sha256: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct VerifyReport {
    pub slots: Vec<VerifySlotReport>,
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct VerifySlotReport {
    pub slot: u16,
    pub image: PathBuf,
    pub local_sha256: String,
    pub device_sha256: String,
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImageInspection {
    pub path: PathBuf,
    pub valid_fat12_1440: bool,
    pub label: Option<String>,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GotekDeviceCheck {
    pub device: PathBuf,
    pub size_bytes: Option<u64>,
    pub slot_count: Option<u64>,
    pub is_regular_file: bool,
    pub slot0_valid_fat12_1440: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct GotekDeviceCandidate {
    pub path: PathBuf,
    pub label: String,
    pub check: GotekDeviceCheck,
}

pub fn gotek_root() -> PathBuf {
    std::env::var_os("GOTEK_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("gotek"))
}

pub fn init_workspace(root: &Path) -> Result<()> {
    fs::create_dir_all(root).with_context(|| format!("creating {}", root.display()))?;
    let gitignore = root.join(".gitignore");
    if !gitignore.exists() {
        fs::write(
            &gitignore,
            ".DS_Store\n*/.floppy.img\n*/.floppy.hash\n*/.floppy.written.hash\n",
        )
        .with_context(|| format!("writing {}", gitignore.display()))?;
    }
    let gotekignore = root.join(".gotekignore");
    if !gotekignore.exists() {
        fs::write(
            &gotekignore,
            "# One glob per line. Matches basename and slot-relative paths.\n# Example:\n# *.json\n# *.svg\n",
        )
        .with_context(|| format!("writing {}", gotekignore.display()))?;
    }
    Ok(())
}

pub fn create_blank_image(path: &Path, label: Option<&str>) -> Result<()> {
    let image = build_fat12_image(&[], label)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(path, image).with_context(|| format!("writing {}", path.display()))
}

pub fn create_designer_disk_image(
    root: &Path,
    paths: &[PathBuf],
    output: &Path,
    label: Option<&str>,
) -> Result<()> {
    if paths.is_empty() {
        bail!("no disk files were provided; generate disk files first");
    }
    let mut entries = Vec::new();
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    for path in paths {
        let rel = path
            .strip_prefix(root)
            .with_context(|| format!("{} is not under {}", path.display(), root.display()))?;
        let mut current = rel.parent();
        let mut ancestors = Vec::new();
        while let Some(parent) = current {
            if !parent.as_os_str().is_empty() {
                ancestors.push(root.join(parent));
            }
            current = parent.parent();
        }
        ancestors.reverse();
        for ancestor in ancestors {
            if seen.insert(ancestor.clone()) {
                candidates.push(ancestor);
            }
        }
        if seen.insert(path.clone()) {
            candidates.push(path.clone());
        }
    }
    candidates.sort_by_key(|path| path.to_string_lossy().to_ascii_lowercase());
    let mut roots = Vec::new();
    'outer: for candidate in candidates {
        for existing in &roots {
            if candidate.starts_with(existing) {
                continue 'outer;
            }
        }
        roots.push(candidate);
    }
    for entry in roots {
        let rel = entry
            .strip_prefix(root)
            .with_context(|| format!("{} is not under {}", entry.display(), root.display()))?;
        let base = rel.file_name().and_then(OsStr::to_str).unwrap_or("");
        if base.starts_with('.') || rel.as_os_str().is_empty() {
            continue;
        }
        collect_image_entries(root, &entry, &mut entries)?;
    }
    let image = build_fat12_image(&entries, label)?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(output, image).with_context(|| format!("writing {}", output.display()))
}

pub fn pack_workspace(options: &GotekOptions) -> Result<PackReport> {
    fs::create_dir_all(&options.root)
        .with_context(|| format!("creating {}", options.root.display()))?;
    let ignore = IgnoreRules::load(&options.root)?;
    let mut slots = Vec::new();
    for slot_dir in slot_dirs(&options.root)? {
        let slot = parse_slot_dir(&slot_dir)?;
        let report = pack_slot_dir(&options.root, &slot_dir, slot, &ignore)?;
        slots.push(report);
    }
    Ok(PackReport {
        root: options.root.clone(),
        slots,
    })
}

pub fn write_workspace_slots(
    device: &Path,
    options: &GotekOptions,
    selected: &[u16],
) -> Result<Vec<SlotIoReport>> {
    let mut reports = Vec::new();
    let ignore = IgnoreRules::load(&options.root)?;
    let selected: HashSet<u16> = selected.iter().copied().collect();
    for slot_dir in slot_dirs(&options.root)? {
        let slot = parse_slot_dir(&slot_dir)?;
        if !selected.is_empty() && !selected.contains(&slot) {
            continue;
        }
        let Some(image) = resolve_slot_image(&options.root, &slot_dir, &ignore)? else {
            continue;
        };
        let image_hash = sha256_file(&image)?;
        let written_hash_file = slot_dir.join(WRITTEN_HASH);
        if read_trimmed(&written_hash_file).as_deref() == Some(image_hash.as_str()) {
            reports.push(SlotIoReport {
                slot,
                image,
                offset: slot_offset(slot),
                bytes: FLOPPY_SIZE,
                sha256: image_hash,
                status: "already-written".to_owned(),
            });
            continue;
        }
        let report = write_slot(device, slot, &image)?;
        fs::write(&written_hash_file, format!("{}\n", image_hash))
            .with_context(|| format!("writing {}", written_hash_file.display()))?;
        reports.push(report);
    }
    Ok(reports)
}

pub fn verify_workspace_slots(
    device: &Path,
    options: &GotekOptions,
    selected: &[u16],
) -> Result<VerifyReport> {
    let ignore = IgnoreRules::load(&options.root)?;
    let selected: HashSet<u16> = selected.iter().copied().collect();
    let mut slots = Vec::new();
    for slot_dir in slot_dirs(&options.root)? {
        let slot = parse_slot_dir(&slot_dir)?;
        if !selected.is_empty() && !selected.contains(&slot) {
            continue;
        }
        let Some(image) = resolve_slot_image(&options.root, &slot_dir, &ignore)? else {
            continue;
        };
        let local_sha256 = sha256_file(&image)?;
        let data = read_slot_bytes(device, slot)?;
        let device_sha256 = sha256_bytes(&data);
        let ok = local_sha256 == device_sha256;
        slots.push(VerifySlotReport {
            slot,
            image,
            local_sha256,
            device_sha256,
            ok,
        });
    }
    let ok = slots.iter().all(|slot| slot.ok);
    Ok(VerifyReport { slots, ok })
}

pub fn write_slot(device: &Path, slot: u16, image: &Path) -> Result<SlotIoReport> {
    validate_slot(slot)?;
    check_gotek_device_for_slot(device, slot)?;
    let data = fs::read(image).with_context(|| format!("reading {}", image.display()))?;
    if data.len() != FLOPPY_SIZE {
        bail!(
            "{} is {} bytes, expected {}",
            image.display(),
            data.len(),
            FLOPPY_SIZE
        );
    }
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(device)
        .with_context(|| format!("opening {}", device.display()))?;
    let offset = slot_offset(slot);
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(&data)?;
    // FlashFloppy's Gotek image collection leaves a 96 KiB zero-filled pad after each 1.44MB slot.
    file.write_all(&vec![0u8; SLOT_PADDING])?;
    file.flush()?;
    Ok(SlotIoReport {
        slot,
        image: image.to_path_buf(),
        offset,
        bytes: data.len(),
        sha256: sha256_bytes(&data),
        status: "wrote".to_owned(),
    })
}

pub fn read_slot(device: &Path, slot: u16, output: &Path) -> Result<SlotIoReport> {
    let data = read_slot_bytes(device, slot)?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(output, &data).with_context(|| format!("writing {}", output.display()))?;
    Ok(SlotIoReport {
        slot,
        image: output.to_path_buf(),
        offset: slot_offset(slot),
        bytes: data.len(),
        sha256: sha256_bytes(&data),
        status: "read".to_owned(),
    })
}

pub fn verify_slot(device: &Path, slot: u16, image: &Path) -> Result<VerifySlotReport> {
    validate_slot(slot)?;
    let local = fs::read(image).with_context(|| format!("reading {}", image.display()))?;
    if local.len() != FLOPPY_SIZE {
        bail!(
            "{} is {} bytes, expected {}",
            image.display(),
            local.len(),
            FLOPPY_SIZE
        );
    }
    let device_data = read_slot_bytes(device, slot)?;
    let local_sha256 = sha256_bytes(&local);
    let device_sha256 = sha256_bytes(&device_data);
    Ok(VerifySlotReport {
        slot,
        image: image.to_path_buf(),
        ok: local_sha256 == device_sha256,
        local_sha256,
        device_sha256,
    })
}

pub fn inspect_image(path: &Path) -> Result<ImageInspection> {
    let data = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let valid = is_valid_fat12_1440(&data);
    let (label, files) = if valid {
        read_root_entries(&data)
    } else {
        (None, Vec::new())
    };
    Ok(ImageInspection {
        path: path.to_path_buf(),
        valid_fat12_1440: valid,
        label,
        files,
    })
}

pub fn check_gotek_device(device: &Path) -> Result<GotekDeviceCheck> {
    let mut file = File::open(device).with_context(|| format!("opening {}", device.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("stat {}", device.display()))?;
    let is_regular_file = metadata.is_file();
    let size_bytes = (metadata.len() > 0).then_some(metadata.len());
    if is_regular_file && size_bytes.unwrap_or(0) < FLOPPY_SIZE as u64 {
        bail!(
            "{} is {} bytes, smaller than one 1.44MB Gotek slot",
            device.display(),
            size_bytes.unwrap_or(0)
        );
    }
    let mut slot0 = vec![0u8; FLOPPY_SIZE];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut slot0)?;
    let slot0_valid_fat12_1440 = is_valid_fat12_1440(&slot0);
    if !slot0_valid_fat12_1440 {
        bail!(
            "{} does not look like an initialized Gotek device: slot 0 is not a valid 1.44MB FAT12 floppy image",
            device.display()
        );
    }
    Ok(GotekDeviceCheck {
        device: device.to_path_buf(),
        size_bytes,
        slot_count: size_bytes.map(|size| size / FLOPPY_SIZE as u64),
        is_regular_file,
        slot0_valid_fat12_1440,
    })
}

fn check_gotek_device_for_slot(device: &Path, slot: u16) -> Result<GotekDeviceCheck> {
    let check = check_gotek_device(device)?;
    let required = (slot as u64 + 1) * FLOPPY_SIZE as u64;
    if check.is_regular_file && check.size_bytes.unwrap_or(0) < required {
        bail!(
            "{} is {} bytes, but slot {slot} requires at least {required} bytes",
            device.display(),
            check.size_bytes.unwrap_or(0)
        );
    }
    Ok(check)
}

pub fn list_gotek_device_candidates() -> Vec<GotekDeviceCandidate> {
    platform_gotek_device_candidates()
}

#[cfg(target_os = "linux")]
fn platform_gotek_device_candidates() -> Vec<GotekDeviceCandidate> {
    let mut candidates = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/block") else {
        return candidates;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("loop") || name.starts_with("ram") {
            continue;
        }
        let path = PathBuf::from("/dev").join(&name);
        let Ok(check) = check_gotek_device(&path) else {
            continue;
        };
        let removable = fs::read_to_string(entry.path().join("removable"))
            .ok()
            .is_some_and(|value| value.trim() == "1");
        let model = fs::read_to_string(entry.path().join("device/model"))
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        let size = check
            .slot_count
            .map(|slots| format!("{slots} slot(s)"))
            .unwrap_or_else(|| "raw block device".to_owned());
        let label = match (removable, model) {
            (true, Some(model)) => format!("{} - {} ({size})", path.display(), model),
            (true, None) => format!("{} - removable ({size})", path.display()),
            (false, Some(model)) => format!("{} - {} ({size})", path.display(), model),
            (false, None) => format!("{} ({size})", path.display()),
        };
        candidates.push(GotekDeviceCandidate { path, label, check });
    }
    candidates.sort_by(|a, b| a.path.cmp(&b.path));
    candidates
}

#[cfg(not(target_os = "linux"))]
fn platform_gotek_device_candidates() -> Vec<GotekDeviceCandidate> {
    Vec::new()
}

fn pack_slot_dir(
    root: &Path,
    slot_dir: &Path,
    slot: u16,
    ignore: &IgnoreRules,
) -> Result<PackSlotReport> {
    let visible_imgs = visible_imgs(root, slot_dir, ignore)?;
    let packable_files = packable_files(root, slot_dir, ignore)?;
    let other_entries = non_ignored_non_image_entries(root, slot_dir, ignore)?;
    let hashfile = slot_dir.join(MANAGED_HASH);
    let current_hash = hash_slot(root, slot_dir, ignore)?;
    let previous_hash = read_trimmed(&hashfile);

    if visible_imgs.len() > 1 {
        bail!("slot {slot:03}: multiple visible .img files found");
    }
    if let Some(image) = visible_imgs.first() {
        if !other_entries.is_empty() {
            bail!("slot {slot:03}: visible .img plus non-image files is ambiguous");
        }
        if previous_hash.as_deref() == Some(current_hash.as_str()) {
            return Ok(PackSlotReport {
                slot,
                status: "unchanged-manual-image".to_owned(),
                image: Some(image.clone()),
                hash: Some(current_hash),
            });
        }
        fs::write(&hashfile, format!("{current_hash}\n"))?;
        return Ok(PackSlotReport {
            slot,
            status: "manual-image-selected".to_owned(),
            image: Some(image.clone()),
            hash: Some(current_hash),
        });
    }

    if packable_files.is_empty() {
        return Ok(PackSlotReport {
            slot,
            status: "empty".to_owned(),
            image: None,
            hash: None,
        });
    }
    if previous_hash.as_deref() == Some(current_hash.as_str()) {
        return Ok(PackSlotReport {
            slot,
            status: "unchanged".to_owned(),
            image: Some(slot_dir.join(MANAGED_IMAGE)),
            hash: Some(current_hash),
        });
    }

    let roots = packable_roots(root, slot_dir, ignore)?;
    let mut entries = Vec::new();
    for entry in roots {
        collect_image_entries(slot_dir, &entry, &mut entries)?;
    }
    let label = read_trimmed(&slot_dir.join(".label"));
    let image = build_fat12_image(&entries, label.as_deref())?;
    let image_path = slot_dir.join(MANAGED_IMAGE);
    fs::write(&image_path, image).with_context(|| format!("writing {}", image_path.display()))?;
    fs::write(&hashfile, format!("{current_hash}\n"))?;
    Ok(PackSlotReport {
        slot,
        status: "packed".to_owned(),
        image: Some(image_path),
        hash: Some(current_hash),
    })
}

#[derive(Debug)]
struct ImageEntry {
    image_path: String,
    source: PathBuf,
    is_dir: bool,
    timestamp: FatTimestamp,
}

fn collect_image_entries(base: &Path, entry: &Path, out: &mut Vec<ImageEntry>) -> Result<()> {
    let rel = entry.strip_prefix(base).unwrap_or(entry);
    let image_path = path_to_image_path(rel)?;
    let meta = fs::metadata(entry).with_context(|| format!("stat {}", entry.display()))?;
    let timestamp = FatTimestamp::from_system_time(meta.modified().ok());
    if meta.is_dir() {
        out.push(ImageEntry {
            image_path: image_path.clone(),
            source: entry.to_path_buf(),
            is_dir: true,
            timestamp,
        });
        let children = fs::read_dir(entry)?
            .map(|item| item.map(|e| e.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        for child in children {
            collect_image_entries(base, &child, out)?;
        }
    } else if meta.is_file() {
        out.push(ImageEntry {
            image_path,
            source: entry.to_path_buf(),
            is_dir: false,
            timestamp,
        });
    }
    Ok(())
}

fn build_fat12_image(entries: &[ImageEntry], label: Option<&str>) -> Result<Vec<u8>> {
    let mut image = vec![0u8; FLOPPY_SIZE];
    write_boot_sector(&mut image, label);
    let mut builder = FatBuilder::new(image);
    if let Some(label) = label.and_then(volume_label_11) {
        builder.add_volume_label(&label, FatTimestamp::now())?;
    }
    for entry in entries {
        if entry.is_dir {
            builder.add_dir(&entry.image_path, entry.timestamp)?;
        } else {
            let data = fs::read(&entry.source)
                .with_context(|| format!("reading {}", entry.source.display()))?;
            builder.add_file(&entry.image_path, &data, entry.timestamp)?;
        }
    }
    builder.finish()
}

struct FatBuilder {
    image: Vec<u8>,
    next_cluster: u16,
    dirs: BTreeMap<String, u16>,
}

impl FatBuilder {
    fn new(image: Vec<u8>) -> Self {
        let mut this = Self {
            image,
            next_cluster: 2,
            dirs: BTreeMap::new(),
        };
        this.set_fat(0, 0xff0);
        this.set_fat(1, 0xfff);
        this
    }

    fn finish(mut self) -> Result<Vec<u8>> {
        let fat_start = BYTES_PER_SECTOR;
        let fat_len = SECTORS_PER_FAT as usize * BYTES_PER_SECTOR;
        let fat1 = self.image[fat_start..fat_start + fat_len].to_vec();
        let fat2_start = fat_start + fat_len;
        self.image[fat2_start..fat2_start + fat_len].copy_from_slice(&fat1);
        Ok(self.image)
    }

    fn add_volume_label(&mut self, label: &[u8; 11], timestamp: FatTimestamp) -> Result<()> {
        let mut entry = [0u8; 32];
        entry[0..11].copy_from_slice(label);
        entry[11] = 0x08;
        timestamp.write_to_dir_entry(&mut entry);
        self.write_dir_entry(None, &entry)
    }

    fn add_dir(&mut self, path: &str, timestamp: FatTimestamp) -> Result<()> {
        let (parent, name) = split_image_path(path);
        if self.dirs.contains_key(path) {
            return Ok(());
        }
        let cluster = self.alloc_chain(1)?;
        self.zero_cluster(cluster);
        let parent_cluster = parent
            .and_then(|parent| self.dirs.get(parent).copied())
            .unwrap_or(0);
        self.init_dir_cluster(cluster, parent_cluster, timestamp);
        self.dirs.insert(path.to_owned(), cluster);
        let mut entry = [0u8; 32];
        entry[0..11].copy_from_slice(&short_name_11(name)?);
        entry[11] = 0x10;
        timestamp.write_to_dir_entry(&mut entry);
        write_u16(&mut entry[26..28], cluster);
        self.write_dir_entry(parent, &entry)
    }

    fn add_file(&mut self, path: &str, data: &[u8], timestamp: FatTimestamp) -> Result<()> {
        let (parent, name) = split_image_path(path);
        let clusters_needed = data.len().div_ceil(BYTES_PER_SECTOR).max(1);
        let first_cluster = self.alloc_chain(clusters_needed)?;
        for (idx, chunk) in data.chunks(BYTES_PER_SECTOR).enumerate() {
            let cluster = first_cluster + idx as u16;
            let offset = cluster_offset(cluster);
            self.image[offset..offset + chunk.len()].copy_from_slice(chunk);
        }
        let mut entry = [0u8; 32];
        entry[0..11].copy_from_slice(&short_name_11(name)?);
        entry[11] = 0x20;
        timestamp.write_to_dir_entry(&mut entry);
        write_u16(&mut entry[26..28], first_cluster);
        write_u32(&mut entry[28..32], data.len() as u32);
        self.write_dir_entry(parent, &entry)
    }

    fn alloc_chain(&mut self, count: usize) -> Result<u16> {
        if count == 0 {
            bail!("cannot allocate empty FAT chain");
        }
        let first = self.next_cluster;
        let last = first + count as u16 - 1;
        if last >= DATA_CLUSTER_COUNT + 2 {
            bail!("floppy image is full");
        }
        for cluster in first..=last {
            let value = if cluster == last { 0xfff } else { cluster + 1 };
            self.set_fat(cluster, value);
        }
        self.next_cluster = last + 1;
        Ok(first)
    }

    fn set_fat(&mut self, cluster: u16, value: u16) {
        let offset = BYTES_PER_SECTOR + (cluster as usize * 3) / 2;
        let value = value & 0x0fff;
        if cluster % 2 == 0 {
            self.image[offset] = (value & 0x00ff) as u8;
            self.image[offset + 1] = (self.image[offset + 1] & 0xf0) | ((value >> 8) as u8 & 0x0f);
        } else {
            self.image[offset] = (self.image[offset] & 0x0f) | (((value << 4) as u8) & 0xf0);
            self.image[offset + 1] = (value >> 4) as u8;
        }
    }

    fn zero_cluster(&mut self, cluster: u16) {
        let offset = cluster_offset(cluster);
        self.image[offset..offset + BYTES_PER_SECTOR].fill(0);
    }

    fn init_dir_cluster(&mut self, cluster: u16, parent_cluster: u16, timestamp: FatTimestamp) {
        let offset = cluster_offset(cluster);

        let mut dot = [0u8; 32];
        dot[0..11].copy_from_slice(b".          ");
        dot[11] = 0x10;
        timestamp.write_to_dir_entry(&mut dot);
        write_u16(&mut dot[26..28], cluster);
        self.image[offset..offset + 32].copy_from_slice(&dot);

        let mut dotdot = [0u8; 32];
        dotdot[0..11].copy_from_slice(b"..         ");
        dotdot[11] = 0x10;
        timestamp.write_to_dir_entry(&mut dotdot);
        write_u16(&mut dotdot[26..28], parent_cluster);
        self.image[offset + 32..offset + 64].copy_from_slice(&dotdot);
    }

    fn write_dir_entry(&mut self, parent: Option<&str>, entry: &[u8; 32]) -> Result<()> {
        let (start, len) = if let Some(parent) = parent {
            let cluster = self
                .dirs
                .get(parent)
                .copied()
                .with_context(|| format!("parent directory {parent:?} has not been created"))?;
            (cluster_offset(cluster), BYTES_PER_SECTOR)
        } else {
            (
                FIRST_ROOT_SECTOR * BYTES_PER_SECTOR,
                ROOT_DIR_SECTORS * BYTES_PER_SECTOR,
            )
        };
        for offset in (start..start + len).step_by(32) {
            if self.image[offset] == 0x00 || self.image[offset] == 0xe5 {
                self.image[offset..offset + 32].copy_from_slice(entry);
                return Ok(());
            }
        }
        bail!("directory is full")
    }
}

fn write_boot_sector(image: &mut [u8], label: Option<&str>) {
    let boot = &mut image[..BYTES_PER_SECTOR];
    boot[0..3].copy_from_slice(&[0xeb, 0x3c, 0x90]);
    boot[3..11].copy_from_slice(b"MTOO4049");
    write_u16(&mut boot[11..13], BYTES_PER_SECTOR as u16);
    boot[13] = SECTORS_PER_CLUSTER;
    write_u16(&mut boot[14..16], RESERVED_SECTORS);
    boot[16] = FAT_COUNT;
    write_u16(&mut boot[17..19], ROOT_ENTRY_COUNT);
    write_u16(&mut boot[19..21], TOTAL_SECTORS);
    boot[21] = 0xf0;
    write_u16(&mut boot[22..24], SECTORS_PER_FAT);
    write_u16(&mut boot[24..26], 18);
    write_u16(&mut boot[26..28], 2);
    boot[36] = 0x00;
    boot[38] = 0x29;
    write_u32(&mut boot[39..43], volume_serial());
    let volume = label.and_then(volume_label_11).unwrap_or(*b"NO NAME    ");
    boot[43..54].copy_from_slice(&volume);
    boot[54..62].copy_from_slice(b"FAT12   ");
    boot[62..110].copy_from_slice(&[
        0xfa, 0x31, 0xc0, 0x8e, 0xd8, 0x8e, 0xc0, 0xfc, 0xb9, 0x00, 0x01, 0xbe, 0x00, 0x7c, 0xbf,
        0x00, 0x80, 0xf3, 0xa5, 0xea, 0x56, 0x00, 0x00, 0x08, 0xb8, 0x01, 0x02, 0xbb, 0x00, 0x7c,
        0xba, 0x80, 0x00, 0xb9, 0x01, 0x00, 0xcd, 0x13, 0x72, 0x05, 0xea, 0x00, 0x7c, 0x00, 0x00,
        0xcd, 0x19, 0x00,
    ]);
    boot[446..462].copy_from_slice(&[
        0x80, 0x00, 0x01, 0x00, 0x01, 0x01, 0x12, 0x4f, 0x00, 0x00, 0x00, 0x00, 0x40, 0x0b, 0x00,
        0x00,
    ]);
    boot[510] = 0x55;
    boot[511] = 0xaa;
}

fn is_valid_fat12_1440(data: &[u8]) -> bool {
    data.len() == FLOPPY_SIZE
        && data[510] == 0x55
        && data[511] == 0xaa
        && u16::from_le_bytes([data[11], data[12]]) == BYTES_PER_SECTOR as u16
        && u16::from_le_bytes([data[19], data[20]]) == TOTAL_SECTORS
        && data[54..62] == *b"FAT12   "
}

fn read_root_entries(data: &[u8]) -> (Option<String>, Vec<String>) {
    let mut label = None;
    let mut files = Vec::new();
    let root_start = FIRST_ROOT_SECTOR * BYTES_PER_SECTOR;
    let root_end = root_start + ROOT_DIR_SECTORS * BYTES_PER_SECTOR;
    for entry in data[root_start..root_end].chunks(32) {
        if entry[0] == 0x00 {
            break;
        }
        if entry[0] == 0xe5 {
            continue;
        }
        let attrs = entry[11];
        if attrs & 0x08 != 0 {
            label = Some(decode_short_name(&entry[0..11]));
        } else if attrs & 0x0f != 0x0f {
            files.push(decode_short_name(&entry[0..11]));
        }
    }
    (label, files)
}

#[derive(Debug, Clone, Copy)]
struct FatTimestamp {
    time: u16,
    date: u16,
}

impl FatTimestamp {
    fn now() -> Self {
        Self::from_system_time(Some(SystemTime::now()))
    }

    fn from_system_time(time: Option<SystemTime>) -> Self {
        let seconds = time
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs())
            .unwrap_or(315_532_800);
        Self::from_unix_seconds(seconds)
    }

    fn from_unix_seconds(seconds: u64) -> Self {
        let days = seconds / 86_400;
        let day_seconds = seconds % 86_400;
        let (year, month, day) = ymd_from_unix_days(days);
        let year = year.clamp(1980, 2107) as u16;
        let month = month.clamp(1, 12) as u16;
        let day = day.clamp(1, 31) as u16;
        let hour = (day_seconds / 3600) as u16;
        let minute = ((day_seconds % 3600) / 60) as u16;
        let second = ((day_seconds % 60) / 2) as u16;

        Self {
            time: (hour << 11) | (minute << 5) | second,
            date: ((year - 1980) << 9) | (month << 5) | day,
        }
    }

    fn write_to_dir_entry(self, entry: &mut [u8; 32]) {
        write_u16(&mut entry[14..16], self.time);
        write_u16(&mut entry[16..18], self.date);
        write_u16(&mut entry[18..20], self.date);
        write_u16(&mut entry[22..24], self.time);
        write_u16(&mut entry[24..26], self.date);
    }
}

fn ymd_from_unix_days(days_since_epoch: u64) -> (i32, u32, u32) {
    let z = days_since_epoch as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    (year as i32, month as u32, day as u32)
}

fn volume_serial() -> u32 {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as u32)
        .unwrap_or(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos())
        .unwrap_or(0);
    seconds.rotate_left(16) ^ nanos
}

fn split_image_path(path: &str) -> (Option<&str>, &str) {
    if let Some((parent, name)) = path.rsplit_once('/') {
        (Some(parent), name)
    } else {
        (None, path)
    }
}

fn path_to_image_path(path: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        let name = component.as_os_str().to_string_lossy();
        if name.is_empty() || name == "." {
            continue;
        }
        short_name_11(&name)?;
        parts.push(name.to_string());
    }
    if parts.is_empty() {
        bail!("empty image path");
    }
    Ok(parts.join("/"))
}

fn short_name_11(name: &str) -> Result<[u8; 11]> {
    let name = name.trim();
    let (stem, ext) = if let Some((stem, ext)) = name.rsplit_once('.') {
        (stem, ext)
    } else {
        (name, "")
    };
    if stem.is_empty() || stem.len() > 8 || ext.len() > 3 {
        bail!("{name:?} is not an 8.3 FAT filename");
    }
    let mut out = *b"           ";
    for (idx, byte) in stem.bytes().enumerate() {
        out[idx] = fat_name_byte(byte, name)?;
    }
    for (idx, byte) in ext.bytes().enumerate() {
        out[8 + idx] = fat_name_byte(byte, name)?;
    }
    Ok(out)
}

fn volume_label_11(label: &str) -> Option<[u8; 11]> {
    let label = label.trim();
    if label.is_empty() || label.len() > 11 {
        return None;
    }
    let mut out = *b"           ";
    for (idx, byte) in label.bytes().enumerate() {
        if !(byte.is_ascii_alphanumeric() || matches!(byte, b' ' | b'_' | b'-')) {
            return None;
        }
        out[idx] = byte.to_ascii_uppercase();
    }
    Some(out)
}

fn fat_name_byte(byte: u8, original: &str) -> Result<u8> {
    if byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'$' | b'%'
                | b'\''
                | b'-'
                | b'_'
                | b'@'
                | b'~'
                | b'`'
                | b'!'
                | b'('
                | b')'
                | b'{'
                | b'}'
                | b'^'
                | b'#'
                | b'&'
        )
    {
        Ok(byte.to_ascii_uppercase())
    } else {
        bail!("{original:?} contains a character unsupported by 8.3 FAT names")
    }
}

fn decode_short_name(raw: &[u8]) -> String {
    let stem = String::from_utf8_lossy(&raw[0..8]).trim_end().to_owned();
    let ext = String::from_utf8_lossy(&raw[8..11]).trim_end().to_owned();
    if ext.is_empty() {
        stem
    } else {
        format!("{stem}.{ext}")
    }
}

fn cluster_offset(cluster: u16) -> usize {
    (FIRST_DATA_SECTOR + (cluster as usize - 2)) * BYTES_PER_SECTOR
}

fn write_u16(out: &mut [u8], value: u16) {
    out.copy_from_slice(&value.to_le_bytes());
}

fn write_u32(out: &mut [u8], value: u32) {
    out.copy_from_slice(&value.to_le_bytes());
}

fn slot_offset(slot: u16) -> u64 {
    slot as u64 * SLOT_STRIDE as u64
}

fn validate_slot(slot: u16) -> Result<()> {
    if slot >= SLOT_COUNT {
        bail!("slot must be 0..{}, got {slot}", SLOT_COUNT - 1);
    }
    Ok(())
}

fn read_slot_bytes(device: &Path, slot: u16) -> Result<Vec<u8>> {
    validate_slot(slot)?;
    let mut file = File::open(device).with_context(|| format!("opening {}", device.display()))?;
    file.seek(SeekFrom::Start(slot_offset(slot)))?;
    let mut data = vec![0u8; FLOPPY_SIZE];
    file.read_exact(&mut data)?;
    Ok(data)
}

fn sha256_file(path: &Path) -> Result<String> {
    let data = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(sha256_bytes(&data))
}

fn sha256_bytes(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn slot_dirs(root: &Path) -> Result<Vec<PathBuf>> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut dirs = Vec::new();
    for entry in fs::read_dir(root).with_context(|| format!("reading {}", root.display()))? {
        let path = entry?.path();
        if path.is_dir()
            && path
                .file_name()
                .and_then(OsStr::to_str)
                .is_some_and(is_slot_name)
        {
            dirs.push(path);
        }
    }
    dirs.sort();
    Ok(dirs)
}

fn is_slot_name(name: &str) -> bool {
    name.len() == 3 && name.bytes().all(|b| b.is_ascii_digit())
}

fn parse_slot_dir(path: &Path) -> Result<u16> {
    path.file_name()
        .and_then(OsStr::to_str)
        .with_context(|| format!("invalid slot directory {}", path.display()))?
        .parse()
        .with_context(|| format!("invalid slot directory {}", path.display()))
}

fn visible_imgs(root: &Path, slot_dir: &Path, ignore: &IgnoreRules) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in immediate_entries(slot_dir)? {
        let base = entry.file_name().and_then(OsStr::to_str).unwrap_or("");
        if entry.is_file()
            && !base.starts_with('.')
            && base.to_ascii_lowercase().ends_with(".img")
            && !ignore.matches(root, slot_dir, &entry)
        {
            out.push(entry);
        }
    }
    out.sort();
    Ok(out)
}

fn packable_files(root: &Path, slot_dir: &Path, ignore: &IgnoreRules) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in recursive_entries(slot_dir)? {
        let base = entry.file_name().and_then(OsStr::to_str).unwrap_or("");
        if !entry.is_file() || is_managed_or_hidden(base) || base.eq(".label") {
            continue;
        }
        if base.to_ascii_lowercase().ends_with(".img") || ignore.matches(root, slot_dir, &entry) {
            continue;
        }
        out.push(entry);
    }
    out.sort();
    Ok(out)
}

fn packable_roots(root: &Path, slot_dir: &Path, ignore: &IgnoreRules) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in immediate_entries(slot_dir)? {
        let base = entry.file_name().and_then(OsStr::to_str).unwrap_or("");
        if is_managed_or_hidden(base)
            || base.eq(".label")
            || base.to_ascii_lowercase().ends_with(".img")
        {
            continue;
        }
        if ignore.matches(root, slot_dir, &entry) {
            continue;
        }
        out.push(entry);
    }
    out.sort();
    Ok(out)
}

fn non_ignored_non_image_entries(
    root: &Path,
    slot_dir: &Path,
    ignore: &IgnoreRules,
) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in recursive_entries(slot_dir)? {
        let base = entry.file_name().and_then(OsStr::to_str).unwrap_or("");
        if is_managed_or_hidden(base) || base.eq(".label") || ignore.matches(root, slot_dir, &entry)
        {
            continue;
        }
        if entry.is_file() && base.to_ascii_lowercase().ends_with(".img") {
            continue;
        }
        out.push(entry);
    }
    out.sort();
    Ok(out)
}

fn is_managed_or_hidden(base: &str) -> bool {
    base.starts_with('.') || matches!(base, MANAGED_IMAGE | MANAGED_HASH | WRITTEN_HASH)
}

fn immediate_entries(path: &Path) -> Result<Vec<PathBuf>> {
    let mut out = fs::read_dir(path)?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    out.sort();
    Ok(out)
}

fn recursive_entries(path: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    fn walk(path: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
        for entry in immediate_entries(path)? {
            out.push(entry.clone());
            if entry.is_dir() {
                walk(&entry, out)?;
            }
        }
        Ok(())
    }
    walk(path, &mut out)?;
    out.sort();
    Ok(out)
}

fn hash_slot(root: &Path, slot_dir: &Path, ignore: &IgnoreRules) -> Result<String> {
    let mut material = Vec::new();
    let label = slot_dir.join(".label");
    if label.is_file() {
        material.extend_from_slice(b".label\n");
        material.extend_from_slice(sha256_file(&label)?.as_bytes());
        material.push(b'\n');
    }
    for entry in recursive_entries(slot_dir)? {
        if !entry.is_file() || ignore.matches(root, slot_dir, &entry) {
            continue;
        }
        let base = entry.file_name().and_then(OsStr::to_str).unwrap_or("");
        if matches!(base, MANAGED_IMAGE | MANAGED_HASH | WRITTEN_HASH) {
            continue;
        }
        let rel = entry
            .strip_prefix(slot_dir)
            .unwrap_or(&entry)
            .to_string_lossy();
        material.extend_from_slice(rel.as_bytes());
        material.push(b'\n');
        material.extend_from_slice(sha256_file(&entry)?.as_bytes());
        material.push(b'\n');
    }
    Ok(sha256_bytes(&material))
}

fn resolve_slot_image(
    root: &Path,
    slot_dir: &Path,
    ignore: &IgnoreRules,
) -> Result<Option<PathBuf>> {
    let visible = visible_imgs(root, slot_dir, ignore)?;
    let other = non_ignored_non_image_entries(root, slot_dir, ignore)?;
    if visible.len() > 1 {
        bail!(
            "slot {}: multiple visible .img files found",
            slot_dir.display()
        );
    }
    if let Some(image) = visible.first() {
        if !other.is_empty() {
            bail!(
                "slot {}: visible .img plus non-image files is ambiguous",
                slot_dir.display()
            );
        }
        return Ok(Some(image.clone()));
    }
    let managed = slot_dir.join(MANAGED_IMAGE);
    Ok(managed.is_file().then_some(managed))
}

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

struct IgnoreRules {
    patterns: Vec<String>,
}

impl IgnoreRules {
    fn load(root: &Path) -> Result<Self> {
        let path = root.join(".gotekignore");
        let contents = fs::read_to_string(&path).unwrap_or_default();
        let patterns = contents
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(str::to_owned)
            .collect();
        Ok(Self { patterns })
    }

    fn matches(&self, _root: &Path, slot_dir: &Path, entry: &Path) -> bool {
        let rel = entry
            .strip_prefix(slot_dir)
            .unwrap_or(entry)
            .to_string_lossy()
            .replace('\\', "/");
        let base = entry.file_name().and_then(OsStr::to_str).unwrap_or("");
        self.patterns
            .iter()
            .any(|pattern| glob_match(pattern, base) || glob_match(pattern, &rel))
    }
}

fn glob_match(pattern: &str, value: &str) -> bool {
    fn inner(pattern: &[u8], value: &[u8]) -> bool {
        if pattern.is_empty() {
            return value.is_empty();
        }
        match pattern[0] {
            b'*' => {
                inner(&pattern[1..], value) || (!value.is_empty() && inner(pattern, &value[1..]))
            }
            b'?' => !value.is_empty() && inner(&pattern[1..], &value[1..]),
            ch => {
                !value.is_empty()
                    && ch.eq_ignore_ascii_case(&value[0])
                    && inner(&pattern[1..], &value[1..])
            }
        }
    }
    inner(pattern.as_bytes(), value.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn creates_valid_blank_image() {
        let image = build_fat12_image(&[], Some("TEST")).unwrap();
        assert_eq!(image.len(), FLOPPY_SIZE);
        assert!(is_valid_fat12_1440(&image));
        let (label, files) = read_root_entries(&image);
        assert_eq!(label.as_deref(), Some("TEST"));
        assert!(files.is_empty());
    }

    #[test]
    fn packs_files_and_writes_reads_slot_bank() {
        let dir = temp_dir("gotek");
        fs::create_dir_all(dir.join("gotek/001/MENU_01")).unwrap();
        fs::write(dir.join("gotek/001/MENU_SEL.PHV"), b"root").unwrap();
        fs::write(dir.join("gotek/001/MENU_01/DES01_01.SHV"), b"design").unwrap();
        let options = GotekOptions {
            root: dir.join("gotek"),
        };
        let report = pack_workspace(&options).unwrap();
        assert_eq!(report.slots[0].status, "packed");

        let bank = dir.join("bank.bin");
        let mut bank_bytes = vec![0u8; FLOPPY_SIZE * 3];
        let slot0 = build_fat12_image(&[], Some("GOTEK")).unwrap();
        bank_bytes[..FLOPPY_SIZE].copy_from_slice(&slot0);
        fs::write(&bank, bank_bytes).unwrap();
        let image = options.root.join("001").join(MANAGED_IMAGE);
        write_slot(&bank, 1, &image).unwrap();
        let verify = verify_slot(&bank, 1, &image).unwrap();
        assert!(verify.ok);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn write_rejects_uninitialized_bank() {
        let dir = temp_dir("gotek_uninitialized");
        let image = dir.join("slot.img");
        let bank = dir.join("bank.bin");
        fs::create_dir_all(&dir).unwrap();
        create_blank_image(&image, Some("TEST")).unwrap();
        fs::write(&bank, vec![0u8; FLOPPY_SIZE * 2]).unwrap();
        let err = write_slot(&bank, 1, &image).unwrap_err();
        assert!(
            err.to_string()
                .contains("does not look like an initialized Gotek device")
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn creates_designer_disk_image_from_generated_layout() {
        let dir = temp_dir("designer_disk_image");
        fs::create_dir_all(dir.join("disk/MENU_01")).unwrap();
        fs::create_dir_all(dir.join("disk/MENU_02")).unwrap();
        fs::write(dir.join("disk/MENU_SEL.PHV"), b"root").unwrap();
        fs::write(dir.join("disk/MENU_01/MENU_01.MHV"), b"menu").unwrap();
        fs::write(dir.join("disk/MENU_01/DES01_01.SHV"), b"design").unwrap();
        fs::write(dir.join("disk/MENU_02/MENU_02.MHV"), b"menu2").unwrap();
        let image = dir.join("slot.img");
        let written = vec![
            dir.join("disk/MENU_SEL.PHV"),
            dir.join("disk/MENU_01"),
            dir.join("disk/MENU_02"),
        ];
        create_designer_disk_image(&dir.join("disk"), &written, &image, Some("DESIGNER1")).unwrap();
        let inspected = inspect_image(&image).unwrap();
        assert!(inspected.valid_fat12_1440);
        assert!(inspected.files.contains(&"MENU_SEL.PHV".to_owned()));
        assert!(inspected.files.contains(&"MENU_01".to_owned()));
        assert!(inspected.files.contains(&"MENU_02".to_owned()));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn subdirectories_include_dot_entries() {
        let dir = temp_dir("dot_entries");
        fs::create_dir_all(dir.join("disk/MENU_01")).unwrap();
        fs::write(dir.join("disk/MENU_SEL.PHV"), b"root").unwrap();
        fs::write(dir.join("disk/MENU_01/MENU_01.MHV"), b"menu").unwrap();
        let image = dir.join("slot.img");
        let written = vec![dir.join("disk/MENU_SEL.PHV"), dir.join("disk/MENU_01")];
        create_designer_disk_image(&dir.join("disk"), &written, &image, Some("DESIGNER1")).unwrap();

        let bytes = fs::read(&image).unwrap();
        let menu_cluster = first_cluster_for_root_name(&bytes, b"MENU_01    ").unwrap();
        let dir_offset = cluster_offset(menu_cluster);
        assert_eq!(&bytes[dir_offset..dir_offset + 11], b".          ");
        assert_eq!(&bytes[dir_offset + 32..dir_offset + 43], b"..         ");

        fs::remove_dir_all(dir).unwrap();
    }

    fn first_cluster_for_root_name(image: &[u8], name: &[u8; 11]) -> Option<u16> {
        let root_start = FIRST_ROOT_SECTOR * BYTES_PER_SECTOR;
        let root_end = root_start + ROOT_DIR_SECTORS * BYTES_PER_SECTOR;
        for entry in image[root_start..root_end].chunks(32) {
            if entry[0] == 0x00 {
                break;
            }
            if &entry[0..11] == name {
                return Some(u16::from_le_bytes([entry[26], entry[27]]));
            }
        }
        None
    }

    fn temp_dir(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("designer1_{name}_{nonce}"))
    }
}
