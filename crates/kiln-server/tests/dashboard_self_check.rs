use kiln_server::dashboard_check::{self, CheckError};

const CAPTURE: &str = include_str!("fixtures/dashboard_capture.json");
const HOST_CPU_NULL: &str = include_str!("fixtures/host_cpu_null.json");
const BOXES_FIRST_HOST_CPU_NULL: &str = include_str!("fixtures/boxes_first_host_cpu_null.json");

#[test]
fn passes_on_realistic_capture_fixture() {
    dashboard_check::parse_and_validate(CAPTURE.as_bytes()).expect("capture fixture");
}

#[test]
fn rejects_empty_body() {
    let error = dashboard_check::parse_and_validate(b"").unwrap_err();
    assert!(matches!(error, CheckError::EmptyBody));
}

#[test]
fn rejects_null_host_cpu() {
    let error = dashboard_check::parse_and_validate(HOST_CPU_NULL.as_bytes()).unwrap_err();
    assert!(matches!(error, CheckError::HostCpuMissing));
}

#[test]
fn rejects_null_host_cpu_when_box_cpu_is_present() {
    let error =
        dashboard_check::parse_and_validate(BOXES_FIRST_HOST_CPU_NULL.as_bytes()).unwrap_err();
    assert!(matches!(error, CheckError::HostCpuMissing));
}
