//! Choose safe raw-I/O alignment from Windows storage properties and disk geometry.
use super::{FLOPPY_SIZE, SLOT_PADDING};
use anyhow::{Context, Result, ensure};

#[derive(Debug, Clone, Copy)]
pub(super) struct ReportedAlignment {
    pub logical: u32,
    pub physical: u32,
    pub offset: u32,
}

/// Some USB bridges do not implement StorageAccessAlignmentProperty. Geometry supplies
/// the logical sector size in that case; physical alignment is unknown, so buffers and
/// Gotek offsets use at least 4 KiB. Known nonzero physical offsets remain unsupported.
pub(super) fn determine_alignment(
    reported: Result<ReportedAlignment>,
    geometry: impl FnOnce() -> Result<u32>,
) -> Result<usize> {
    let (logical, physical, offset) = match reported {
        Ok(value) if value.logical != 0 => (value.logical, value.physical, value.offset),
        Ok(value) => (
            geometry().context("querying disk geometry for the logical sector size")?,
            value.physical,
            value.offset,
        ),
        Err(error) => (
            geometry().with_context(|| {
                format!("querying disk geometry after the alignment query failed: {error:#}")
            })?,
            0,
            0,
        ),
    };
    ensure!(
        logical.is_power_of_two(),
        "invalid logical sector size: {logical}"
    );
    ensure!(offset == 0, "unsupported physical sector offset: {offset}");
    if physical != 0 {
        ensure!(
            physical.is_power_of_two() && physical >= logical,
            "invalid physical sector size: {physical} (logical sector size {logical})"
        );
    }
    let align = (logical.max(physical).max(4096)) as usize;
    ensure!(
        FLOPPY_SIZE % align == 0 && SLOT_PADDING % align == 0,
        "Gotek slots are not aligned to this disk's {align}-byte sectors"
    );
    Ok(align)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    #[test]
    fn uses_reported_sector_sizes_and_rejects_unsafe_values() {
        let valid = ReportedAlignment {
            logical: 512,
            physical: 4096,
            offset: 0,
        };
        assert_eq!(
            determine_alignment(Ok(valid), || panic!("geometry not needed")).unwrap(),
            4096
        );
        assert!(
            determine_alignment(
                Ok(ReportedAlignment {
                    offset: 512,
                    ..valid
                }),
                || Ok(512)
            )
            .is_err()
        );
        assert!(
            determine_alignment(
                Ok(ReportedAlignment {
                    physical: 1000,
                    ..valid
                }),
                || Ok(512)
            )
            .is_err()
        );
        assert!(
            determine_alignment(
                Ok(ReportedAlignment {
                    logical: 4096,
                    physical: 512,
                    ..valid
                }),
                || Ok(512)
            )
            .is_err()
        );
        assert!(
            determine_alignment(
                Ok(ReportedAlignment {
                    logical: 0,
                    ..valid
                }),
                || Ok(0)
            )
            .is_err()
        );
        assert!(
            determine_alignment(
                Ok(ReportedAlignment {
                    logical: 65536,
                    physical: 65536,
                    ..valid
                }),
                || Ok(512)
            )
            .is_err()
        );
    }

    #[test]
    fn uses_geometry_when_alignment_query_is_unsupported() {
        let actual = determine_alignment(Err(anyhow!("not supported")), || Ok(512)).unwrap();
        assert_eq!(actual, 4096);
        let err = determine_alignment(Err(anyhow!("alignment unsupported")), || {
            Err(anyhow!("geometry unavailable"))
        })
        .unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("alignment unsupported"));
        assert!(message.contains("geometry unavailable"));
    }

    #[test]
    fn handles_missing_physical_and_logical_sizes() {
        let missing_physical = ReportedAlignment {
            logical: 512,
            physical: 0,
            offset: 0,
        };
        assert_eq!(
            determine_alignment(Ok(missing_physical), || panic!("geometry not needed")).unwrap(),
            4096
        );
        let missing_logical = ReportedAlignment {
            logical: 0,
            physical: 4096,
            offset: 0,
        };
        assert_eq!(
            determine_alignment(Ok(missing_logical), || Ok(512)).unwrap(),
            4096
        );
        assert!(
            determine_alignment(
                Ok(ReportedAlignment {
                    offset: 512,
                    ..missing_logical
                }),
                || Ok(512)
            )
            .is_err()
        );
    }
}
