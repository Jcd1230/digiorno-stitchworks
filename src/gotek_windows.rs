//! Windows whole-disk access. Volume handles cannot address the complete Gotek bank.
use super::*;
use anyhow::{Context, Result, ensure};
use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::ffi::OsString;
use std::mem::size_of;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;
use windows_sys::Win32::Foundation::{
    ERROR_NO_MORE_FILES, ERROR_NO_MORE_ITEMS, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::*;

fn ioctl(file: &File, code: u32, input: &[u8], output: &mut [u8]) -> std::io::Result<usize> {
    let mut returned = 0;
    let ok = unsafe {
        DeviceIoControl(
            file.as_raw_handle() as _,
            code,
            if input.is_empty() {
                null()
            } else {
                input.as_ptr().cast()
            },
            input.len() as u32,
            if output.is_empty() {
                null_mut()
            } else {
                output.as_mut_ptr().cast()
            },
            output.len() as u32,
            &mut returned,
            null_mut(),
        )
    };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(returned as usize)
    }
}
fn bytes_mut<T>(value: &mut T) -> &mut [u8] {
    unsafe { std::slice::from_raw_parts_mut((value as *mut T).cast(), size_of::<T>()) }
}
fn query<T: Default>(file: &File, code: u32) -> Result<T> {
    let mut value = T::default();
    let count = ioctl(file, code, &[], bytes_mut(&mut value))?;
    ensure!(
        count >= size_of::<T>(),
        "Windows returned an incomplete device response"
    );
    Ok(value)
}
fn property_bytes(file: &File, id: i32) -> Result<Vec<u8>> {
    let mut query = [0u8; size_of::<STORAGE_PROPERTY_QUERY>()];
    query[..4].copy_from_slice(&id.to_ne_bytes());
    query[4..8].copy_from_slice(&PropertyStandardQuery.to_ne_bytes());
    let mut result = vec![0u8; 4096];
    let count = ioctl(file, IOCTL_STORAGE_QUERY_PROPERTY, &query, &mut result)?;
    result.truncate(count);
    Ok(result)
}
fn property<T: Copy>(file: &File, id: i32) -> Result<T> {
    let result = property_bytes(file, id)?;
    ensure!(
        result.len() >= size_of::<T>(),
        "Windows returned an incomplete storage descriptor"
    );
    Ok(unsafe { std::ptr::read_unaligned(result.as_ptr().cast::<T>()) })
}
fn open_device_for_query(path: &Path) -> Result<File> {
    OpenOptions::new()
        .access_mode(0)
        .open(path)
        .with_context(|| format!("opening {} for identification", path.display()))
}
fn open_raw(path: &Path, write: bool) -> Result<File> {
    OpenOptions::new()
        .read(true)
        .write(write)
        .open(path)
        .with_context(|| {
            format!(
                "opening {}; for raw disk access, run Designer 1 as administrator",
                path.display()
            )
        })
}
fn number(file: &File) -> Result<u32> {
    Ok(query::<STORAGE_DEVICE_NUMBER>(file, IOCTL_STORAGE_GET_DEVICE_NUMBER)?.DeviceNumber)
}
fn disk_path(number: u32) -> PathBuf {
    PathBuf::from(format!(r"\\.\PhysicalDrive{number}"))
}
fn physical_number(path: &Path) -> Option<u32> {
    let s = path.to_string_lossy();
    s.to_ascii_lowercase()
        .strip_prefix(r"\\.\physicaldrive")?
        .parse()
        .ok()
}
pub(super) fn is_raw_device(path: &Path) -> bool {
    super::windows_drive_letter_path(&path.to_string_lossy()).is_some()
        || physical_number(path).is_some()
}
// Dynamically sized because spanned volumes can return ERROR_MORE_DATA.
fn extents(file: &File) -> Result<Vec<DISK_EXTENT>> {
    let mut buf = vec![0u64; 4];
    loop {
        let out =
            unsafe { std::slice::from_raw_parts_mut(buf.as_mut_ptr().cast::<u8>(), buf.len() * 8) };
        match ioctl(file, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, &[], out) {
            Ok(n) => {
                ensure!(n >= 8, "incomplete volume extents");
                let count = buf[0] as u32 as usize;
                let required = 8usize
                    .checked_add(
                        count
                            .checked_mul(size_of::<DISK_EXTENT>())
                            .context("too many volume extents")?,
                    )
                    .context("too many volume extents")?;
                ensure!(count > 0 && n >= required, "incomplete volume extents");
                let ptr = unsafe { (buf.as_ptr().cast::<u8>()).add(8).cast::<DISK_EXTENT>() };
                return Ok(unsafe { std::slice::from_raw_parts(ptr, count) }.to_vec());
            }
            Err(e) if e.raw_os_error() == Some(234) && buf.len() < 8192 => {
                buf.resize(buf.len() * 2, 0)
            }
            Err(e) => return Err(e.into()),
        }
    }
}
fn single_zero_extent(extents: &[DISK_EXTENT]) -> Result<u32> {
    ensure!(
        extents.len() == 1 && extents[0].StartingOffset == 0,
        "volume must have one extent starting at disk offset zero; select the correct whole Gotek disk"
    );
    Ok(extents[0].DiskNumber)
}
pub(super) fn resolve_disk_path(path: &Path) -> Result<PathBuf> {
    if let Some(n) = physical_number(path) {
        return Ok(disk_path(n));
    }
    let letter = super::windows_drive_letter_path(&path.to_string_lossy())
        .context("expected a drive letter or PhysicalDrive path")?;
    let volume = super::windows_raw_volume_path(letter);
    let file = open_device_for_query(&volume)?;
    Ok(disk_path(single_zero_extent(
        &extents(&file)
            .with_context(|| format!("resolving {} to a physical disk", volume.display()))?,
    )?))
}
struct Aligned {
    ptr: *mut u8,
    layout: Layout,
}
impl Aligned {
    fn new(len: usize, align: usize) -> Result<Self> {
        let layout =
            Layout::from_size_align(len, align).context("unsupported disk buffer alignment")?;
        let ptr = unsafe { alloc_zeroed(layout) };
        ensure!(!ptr.is_null(), "could not allocate disk I/O buffer");
        Ok(Self { ptr, layout })
    }
    fn as_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.layout.size()) }
    }
}
impl Drop for Aligned {
    fn drop(&mut self) {
        unsafe { dealloc(self.ptr, self.layout) }
    }
}
fn alignment(file: &File) -> Result<usize> {
    let reported =
        property::<STORAGE_ACCESS_ALIGNMENT_DESCRIPTOR>(file, StorageAccessAlignmentProperty)
            .map(|a| super::device_alignment::ReportedAlignment {
                logical: a.BytesPerLogicalSector,
                physical: a.BytesPerPhysicalSector,
                offset: a.BytesOffsetForSectorAlignment,
            })
            .context("querying disk sector alignment");
    super::device_alignment::determine_alignment(reported, || {
        let geometry = query::<DISK_GEOMETRY>(file, IOCTL_DISK_GET_DRIVE_GEOMETRY)?;
        Ok(geometry.BytesPerSector)
    })
}

fn length(file: &File) -> Result<u64> {
    let len = query::<GET_LENGTH_INFORMATION>(file, IOCTL_DISK_GET_LENGTH_INFO)?.Length;
    ensure!(len > 0, "disk has no accessible capacity");
    Ok(len as u64)
}
fn read_at(file: &mut File, offset: u64, len: usize) -> Result<Vec<u8>> {
    let align = alignment(file)?;
    ensure!(
        offset % align as u64 == 0 && len % align == 0,
        "unaligned raw disk read"
    );
    let mut buf = Aligned::new(len, align)?;
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(buf.as_mut())?;
    Ok(buf.as_mut().to_vec())
}
pub(super) fn read_slot(path: &Path, offset: u64) -> Result<Vec<u8>> {
    let disk = resolve_disk_path(path)?;
    let mut file = open_raw(&disk, false)?;
    let size = length(&file).context("querying disk capacity")?;
    ensure!(
        offset
            .checked_add(FLOPPY_SIZE as u64)
            .is_some_and(|end| end <= size),
        "slot exceeds disk capacity"
    );
    read_at(&mut file, offset, FLOPPY_SIZE)
}
fn probe_device(path: &Path) -> Result<GotekDeviceCheck> {
    let disk = resolve_disk_path(path)?;
    let mut file = open_raw(&disk, false)?;
    let size = length(&file).context("querying disk capacity")?;
    ensure!(
        size >= FLOPPY_SIZE as u64,
        "disk is smaller than one Gotek slot"
    );
    let slot0_valid_fat12_1440 = is_valid_fat12_1440(&read_at(&mut file, 0, FLOPPY_SIZE)?);
    Ok(GotekDeviceCheck {
        device: disk,
        size_bytes: Some(size),
        slot_count: Some(size / SLOT_STRIDE as u64),
        is_regular_file: false,
        slot0_valid_fat12_1440,
    })
}

pub(super) fn check_device(path: &Path) -> Result<GotekDeviceCheck> {
    let check = probe_device(path)?;
    ensure!(
        check.slot0_valid_fat12_1440,
        "{} does not look like an initialized Gotek device: slot 0 is not a valid 1.44MB FAT12 floppy image",
        check.device.display()
    );
    Ok(check)
}

struct VolumeSearch(windows_sys::Win32::Foundation::HANDLE);
impl Drop for VolumeSearch {
    fn drop(&mut self) {
        unsafe {
            FindVolumeClose(self.0);
        }
    }
}
fn volumes() -> Result<Vec<PathBuf>> {
    let mut buf = [0u16; 1024];
    let handle = unsafe { FindFirstVolumeW(buf.as_mut_ptr(), buf.len() as u32) };
    ensure!(
        handle != INVALID_HANDLE_VALUE,
        "enumerating volumes: {}",
        std::io::Error::last_os_error()
    );
    let _guard = VolumeSearch(handle);
    let mut result = Vec::new();
    loop {
        let end = buf
            .iter()
            .position(|c| *c == 0)
            .context("invalid volume name")?;
        let mut name = OsString::from_wide(&buf[..end]);
        // Volume GUID handles must omit the trailing backslash.
        let mut wide: Vec<u16> = name.encode_wide().collect();
        if wide.last() == Some(&(b'\\' as u16)) {
            wide.pop();
        }
        name = OsString::from_wide(&wide);
        result.push(PathBuf::from(name));
        buf.fill(0);
        if unsafe { FindNextVolumeW(handle, buf.as_mut_ptr(), buf.len() as u32) } == 0 {
            let err = std::io::Error::last_os_error();
            // FindNextVolume uses the file-search completion code, unlike SetupDi.
            if err.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                break;
            }
            return Err(err).context("enumerating volumes");
        }
    }
    Ok(result)
}
fn acquire_volumes<T>(
    volumes: Vec<PathBuf>,
    mut acquire: impl FnMut(&Path) -> Result<Option<T>>,
) -> Result<Vec<T>> {
    let mut locks = Vec::new();
    for volume in volumes {
        if let Some(lock) = acquire(&volume)? {
            locks.push(lock);
        }
    }
    Ok(locks)
}

fn volume_locks(disk_number: u32) -> Result<Vec<File>> {
    acquire_volumes(volumes()?, |volume| {
        let mut root: Vec<u16> = volume.as_os_str().encode_wide().collect();
        root.extend([b'\\' as u16, 0]);
        // Optical and network volumes are not backed by a disk interface.
        if matches!(unsafe { GetDriveTypeW(root.as_ptr()) }, 4 | 5) {
            return Ok(None);
        }
        let file = match open_device_for_query(volume) {
            Ok(file) => file,
            // Empty optical drives and inaccessible volumes cannot identify disk extents.
            Err(err) => {
                return Err(err.context(format!(
                    "cannot establish whether {} belongs to the target disk",
                    volume.display()
                )));
            }
        };
        let ex = extents(&file).with_context(|| {
            format!(
                "querying extents of {}; refusing to write without identifying every disk volume",
                volume.display()
            )
        })?;
        if !ex.iter().any(|e| e.DiskNumber == disk_number) {
            return Ok(None);
        }
        ensure!(
            ex.len() == 1 && ex[0].StartingOffset == 0,
            "{} has an unsupported partition or multi-disk extent; refusing to write",
            volume.display()
        );
        drop(file);
        let file = open_raw(volume, true)?;
        let current = extents(&file)?;
        ensure!(
            single_zero_extent(&current)? == disk_number,
            "volume changed while acquiring disk locks"
        );
        ioctl(&file, FSCTL_LOCK_VOLUME, &[], &mut []).with_context(|| {
            format!(
                "locking {}; close files on the device and retry as administrator",
                volume.display()
            )
        })?;
        ioctl(&file, FSCTL_DISMOUNT_VOLUME, &[], &mut [])
            .with_context(|| format!("dismounting {}", volume.display()))?;
        Ok(Some(file))
    })
}
fn raw_write_fits(offset: u64, size: u64) -> bool {
    offset
        .checked_add(SLOT_STRIDE as u64)
        .is_some_and(|end| end <= size)
}
pub(super) fn write_slot(path: &Path, offset: u64, data: &[u8]) -> Result<()> {
    let disk = resolve_disk_path(path)?;
    let disk_number = physical_number(&disk).context("invalid physical disk path")?;
    let initial = open_raw(&disk, false)?;
    let size = length(&initial)?;
    let identity =
        property_bytes(&initial, StorageDeviceProperty).context("identifying target disk")?;
    ensure!(number(&initial)? == disk_number, "disk identity changed");
    ensure!(
        raw_write_fits(offset, size),
        "slot and padding exceed disk capacity"
    );
    let _locks = volume_locks(disk_number)?;
    let mut file = open_raw(&disk, true)?;
    ensure!(
        number(&initial)? == disk_number
            && length(&initial)? == size
            && number(&file)? == disk_number
            && length(&file)? == size
            && property_bytes(&file, StorageDeviceProperty)? == identity,
        "disk identity or capacity changed while acquiring locks"
    );
    ensure!(
        is_valid_fat12_1440(&read_at(&mut file, 0, FLOPPY_SIZE)?),
        "slot 0 changed or is not a valid Gotek image; refusing to write"
    );
    let align = alignment(&file)?;
    ensure!(offset % align as u64 == 0, "unaligned raw disk write");
    let mut buf = Aligned::new(SLOT_STRIDE, align)?;
    buf.as_mut()[..data.len()].copy_from_slice(data);
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(buf.as_mut()).context("writing Gotek slot")?;
    file.sync_all().context("flushing Gotek device")?;
    Ok(())
}

struct DeviceInfoSet(HDEVINFO);
impl Drop for DeviceInfoSet {
    fn drop(&mut self) {
        unsafe {
            SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}
fn usb_disk_numbers() -> Result<(Vec<u32>, bool)> {
    let handle = unsafe {
        SetupDiGetClassDevsW(
            &GUID_DEVINTERFACE_DISK,
            null(),
            null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    };
    ensure!(
        handle != -1,
        "enumerating disks: {}",
        std::io::Error::last_os_error()
    );
    let _guard = DeviceInfoSet(handle);
    let mut disks = Vec::new();
    let mut access_denied = false;
    for index in 0.. {
        let mut interface = SP_DEVICE_INTERFACE_DATA {
            cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
            ..Default::default()
        };
        if unsafe {
            SetupDiEnumDeviceInterfaces(
                handle,
                null(),
                &GUID_DEVINTERFACE_DISK,
                index,
                &mut interface,
            )
        } == 0
        {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(ERROR_NO_MORE_ITEMS as i32) {
                break;
            }
            return Err(err).context("enumerating disk interfaces");
        }
        let mut required = 0;
        unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                handle,
                &interface,
                null_mut(),
                0,
                &mut required,
                null_mut(),
            );
        }
        ensure!(
            required >= size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32,
            "invalid disk interface detail size"
        );
        let mut buf = vec![0u64; (required as usize + 7) / 8];
        let detail = buf.as_mut_ptr().cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
        unsafe {
            (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
        }
        if unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                handle,
                &interface,
                detail,
                required,
                &mut required,
                null_mut(),
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error()).context("reading disk interface");
        }
        let path_ptr = unsafe { std::ptr::addr_of!((*detail).DevicePath).cast::<u16>() };
        let max = (required as usize - 4) / 2;
        let wide = unsafe { std::slice::from_raw_parts(path_ptr, max) };
        let end = wide
            .iter()
            .position(|v| *v == 0)
            .context("unterminated disk interface path")?;
        let path = PathBuf::from(OsString::from_wide(&wide[..end]));
        let file = match open_device_for_query(&path) {
            Ok(file) => file,
            Err(err) => {
                access_denied |= is_access_denied(&err);
                continue;
            }
        };
        let Ok(descriptor) = property_bytes(&file, StorageDeviceProperty) else {
            continue;
        };
        let bus_offset = std::mem::offset_of!(STORAGE_DEVICE_DESCRIPTOR, BusType);
        let Some(bus_bytes) = descriptor.get(bus_offset..bus_offset + size_of::<i32>()) else {
            continue;
        };
        if i32::from_ne_bytes(bus_bytes.try_into().expect("bus type has four bytes")) != BusTypeUsb
        {
            continue;
        }
        if let Ok(n) = number(&file) {
            disks.push(n);
        }
    }
    disks.sort_unstable();
    disks.dedup();
    Ok((disks, access_denied))
}
fn is_access_denied(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::PermissionDenied)
    })
}
fn disk_letters(disk: &Path) -> Vec<String> {
    (b'A'..=b'Z')
        .filter_map(|letter| {
            let path = super::windows_raw_volume_path(letter as char);
            (resolve_disk_path(&path).ok().as_deref() == Some(disk))
                .then(|| format!("{}:", letter as char))
        })
        .collect()
}
pub(super) fn candidates() -> Result<(Vec<GotekDeviceCandidate>, Option<String>)> {
    let mut result = Vec::new();
    let (disks, access_denied) = usb_disk_numbers()?;
    let mut warnings = Vec::new();
    if access_denied {
        warnings.push(
            "Some disks could not be identified; try running Designer 1 as administrator"
                .to_owned(),
        );
    }
    for n in disks {
        let path = disk_path(n);
        let check = match probe_device(&path) {
            Ok(check) if check.slot0_valid_fat12_1440 => check,
            // Ordinary USB disks are expected not to contain a Gotek floppy in slot 0.
            Ok(_) => continue,
            Err(err) => {
                warnings.push(format!("{}: {err:#}", path.display()));
                continue;
            }
        };
        let letters = disk_letters(&path);
        let label = format!(
            "{} - USB {} ({} bytes, {} slots)",
            path.display(),
            if letters.is_empty() {
                "unmounted".into()
            } else {
                letters.join(", ")
            },
            check.size_bytes.unwrap_or(0),
            check.slot_count.unwrap_or(0).min(SLOT_COUNT as u64)
        );
        result.push(GotekDeviceCandidate { path, label, check });
    }
    let warning = (!warnings.is_empty()).then(|| warnings.join("\n"));
    Ok((result, warning))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerates_windows_volumes_to_completion() {
        let paths = volumes().expect("volume enumeration should finish without an error");
        assert!(!paths.is_empty(), "Windows should have a system volume");
        for path in paths {
            let name = path.to_string_lossy();
            assert!(name.to_ascii_lowercase().starts_with(r"\\?\volume{"));
            assert!(
                name.ends_with('}'),
                "volume handles must omit the trailing slash"
            );
        }
    }

    #[test]
    fn write_bounds_include_padding_and_overflow() {
        assert!(!raw_write_fits(0, FLOPPY_SIZE as u64));
        assert!(!raw_write_fits(
            SLOT_STRIDE as u64,
            (2 * SLOT_STRIDE - 1) as u64
        ));
        assert!(raw_write_fits(SLOT_STRIDE as u64, (2 * SLOT_STRIDE) as u64));
        assert!(!raw_write_fits(u64::MAX, u64::MAX));
    }

    #[test]
    fn releases_acquired_locks_if_a_later_lock_fails() {
        use std::cell::Cell;
        use std::rc::Rc;
        struct Lock(Rc<Cell<usize>>);
        impl Drop for Lock {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let dropped = Rc::new(Cell::new(0));
        let mut calls = 0;
        let result = acquire_volumes(vec![PathBuf::from("a"), PathBuf::from("b")], |_| {
            calls += 1;
            if calls == 2 {
                anyhow::bail!("lock failed");
            }
            Ok(Some(Lock(dropped.clone())))
        });
        assert!(result.is_err());
        assert_eq!(dropped.get(), 1);
    }

    #[test]
    fn validates_volume_mapping() {
        let e = DISK_EXTENT {
            DiskNumber: 3,
            StartingOffset: 0,
            ExtentLength: 1234,
        };
        assert_eq!(single_zero_extent(&[e]).unwrap(), 3);
        assert!(single_zero_extent(&[]).is_err());
        assert!(single_zero_extent(&[e, e]).is_err());
        assert!(
            single_zero_extent(&[DISK_EXTENT {
                StartingOffset: 512,
                ..e
            }])
            .is_err()
        );
    }

    #[test]
    fn recognizes_drive_and_physical_paths() {
        for path in [r"D:", r"d:/", r"\\.\D:", r"\\.\PhysicalDrive2"] {
            assert!(is_raw_device(Path::new(path)), "{path}");
        }
        assert!(!is_raw_device(Path::new(r"D:\folder\bank.img")));
        assert!(super::super::gotek_device_paths_match(
            Path::new(r"\\.\PhysicalDrive42"),
            Path::new(r"\\.\PHYSICALDRIVE42")
        ));
        assert_eq!(physical_number(Path::new(r"\\.\PHYSICALDRIVE42")), Some(42));
    }
}
