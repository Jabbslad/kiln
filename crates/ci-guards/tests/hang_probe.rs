//! Sleep probe for hermetic server-check hang-evidence proofs only.

use std::time::Duration;

#[test]
fn hang_probe_sleep() {
    let Some(secs) = std::env::var("KILN_SERVER_CHECK_HANG_PROBE_SECS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
    else {
        return;
    };
    std::thread::sleep(Duration::from_secs(secs));
}
