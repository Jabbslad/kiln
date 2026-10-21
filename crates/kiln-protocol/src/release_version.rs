//! Fixed slot a release workflow can overwrite without rebuilding.
//!
//! The bytes `KILNRELSLOT=` followed by 32 value bytes live in every shipped
//! binary. An empty value means the Cargo.toml version. A tag publish writes
//! `X.Y.Z` into reused binaries so they report the tag.

const SLOT_LEN: usize = 44;
const MARKER_LEN: usize = 12;

#[used]
static RELEASE_SLOT: [u8; SLOT_LEN] = {
    let mut slot = [0u8; SLOT_LEN];
    slot[0] = b'K';
    slot[1] = b'I';
    slot[2] = b'L';
    slot[3] = b'N';
    slot[4] = b'R';
    slot[5] = b'E';
    slot[6] = b'L';
    slot[7] = b'S';
    slot[8] = b'L';
    slot[9] = b'O';
    slot[10] = b'T';
    slot[11] = b'=';
    slot
};

pub fn release_version() -> &'static str {
    static LABEL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    LABEL
        .get_or_init(|| {
            // The publish workflow overwrites this slot in the shipped file.
            // A plain load folds back to the empty initializer and ignores the stamp.
            let slot = unsafe {
                // SAFETY: RELEASE_SLOT is a fixed aligned byte array and this is a read.
                std::ptr::read_volatile(std::ptr::addr_of!(RELEASE_SLOT))
            };
            stamped_version(&slot).unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())
        })
        .as_str()
}

fn has_marker(slot: &[u8]) -> bool {
    slot.len() == SLOT_LEN
        && slot[0] == b'K'
        && slot[1] == b'I'
        && slot[2] == b'L'
        && slot[3] == b'N'
        && slot[4] == b'R'
        && slot[5] == b'E'
        && slot[6] == b'L'
        && slot[7] == b'S'
        && slot[8] == b'L'
        && slot[9] == b'O'
        && slot[10] == b'T'
        && slot[11] == b'='
}

fn stamped_version(slot: &[u8]) -> Option<String> {
    if !has_marker(slot) {
        return None;
    }
    let value = &slot[MARKER_LEN..];
    let end = value
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(value.len());
    let text = std::str::from_utf8(&value[..end]).ok()?;
    if !is_release_version(text) {
        return None;
    }
    Some(text.to_string())
}

fn is_release_version(text: &str) -> bool {
    let mut parts = text.split('.');
    let (Some(major), Some(minor), Some(patch)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    parts.next().is_none() && [major, minor, patch].into_iter().all(is_component)
}

fn is_component(text: &str) -> bool {
    let bytes = text.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 9
        && bytes.iter().all(|byte| byte.is_ascii_digit())
        && (bytes.len() == 1 || bytes[0] != b'0')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_slot_uses_the_cargo_version() {
        assert_eq!(release_version(), env!("CARGO_PKG_VERSION"));
        assert!(stamped_version(&RELEASE_SLOT).is_none());
    }

    #[test]
    fn a_stamped_slot_reports_that_version() {
        let mut slot = RELEASE_SLOT;
        let version = b"9.9.9";
        slot[MARKER_LEN..MARKER_LEN + version.len()].copy_from_slice(version);
        assert_eq!(stamped_version(&slot).as_deref(), Some("9.9.9"));
    }

    #[test]
    fn a_bad_stamp_is_ignored() {
        let mut slot = RELEASE_SLOT;
        slot[MARKER_LEN] = b'x';
        assert!(stamped_version(&slot).is_none());
    }
}
