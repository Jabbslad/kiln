use kiln_server::dashboard_check::{self, CheckError};

const CAPTURE: &str = include_str!("fixtures/dashboard_capture.json");
const CAPTURE_HOST_DISK: &str = include_str!("fixtures/dashboard_capture_host_disk_launch.json");
const HOST_CPU_NULL: &str = include_str!("fixtures/host_cpu_null.json");
const BOXES_FIRST_HOST_CPU_NULL: &str = include_str!("fixtures/boxes_first_host_cpu_null.json");
const RUNNING_BOX_CPU_NULL: &str = include_str!("fixtures/running_box_cpu_null_host_ok.json");
const RUNNING_BOX_MISSING_LAUNCH: &str =
    include_str!("fixtures/running_box_missing_launch_timing.json");
const RUNNING_BOX_LEGACY_LAUNCH: &str =
    include_str!("fixtures/running_box_legacy_launch_timing.json");

#[test]
fn rejects_realistic_capture_fixture_without_host_disk() {
    let error = dashboard_check::parse_and_validate(CAPTURE.as_bytes()).unwrap_err();
    assert!(matches!(error, CheckError::HostDiskMissing));
}

#[test]
fn passes_when_capture_fixture_has_host_disk() {
    dashboard_check::parse_and_validate(CAPTURE_HOST_DISK.as_bytes()).expect("capture with disk");
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

#[test]
fn passes_when_running_box_cpu_is_null_and_host_metrics_are_numeric() {
    dashboard_check::parse_and_validate(RUNNING_BOX_CPU_NULL.as_bytes())
        .expect("deploy gate is host-only");
}

#[test]
fn passes_when_legacy_running_box_has_null_timing() {
    dashboard_check::parse_and_validate(RUNNING_BOX_LEGACY_LAUNCH.as_bytes())
        .expect("legacy running boxes stay exempt until a v2 launch is recorded");
}

#[test]
fn rejects_running_box_without_launch_timing() {
    let error =
        dashboard_check::parse_and_validate(RUNNING_BOX_MISSING_LAUNCH.as_bytes()).unwrap_err();
    assert!(matches!(error, CheckError::RunningLaunchTimingMissing));
}

#[test]
fn passes_when_no_boxes_are_running() {
    let body = br#"{"schema_version":1,"version":"0.4.17","observed_at":"2026-10-21T12:00:01Z","workspace":"local","host":{"name":"local","state":"online","metrics":{"cpu_percent":1.0,"memory_bytes":5000,"disk_bytes":4000,"disk_total_bytes":8000}},"boxes":[{"id":"b1","name":"idle","state":"stopped","image":"","resources":null,"metrics":{"cpu_percent":null,"memory_bytes":null,"disk_bytes":100},"created_at":"2026-10-21T12:00:00Z","last_operation":null,"last_error":null,"restored_from":null,"timing":null}],"templates":[],"checkpoints":[],"operations":[]}"#;
    dashboard_check::parse_and_validate(body).expect("stopped-only inventory");
}

#[test]
fn require_launch_timing_rejects_named_box_with_null_timing() {
    let error =
        dashboard_check::require_launch_timing(RUNNING_BOX_LEGACY_LAUNCH.as_bytes(), "smoke")
            .unwrap_err();
    assert!(matches!(error, CheckError::RequireLaunchTimingMissing));
}

#[test]
fn require_launch_timing_passes_when_named_box_has_total_ms() {
    dashboard_check::require_launch_timing(CAPTURE_HOST_DISK.as_bytes(), "running-box")
        .expect("capture running-box reports total_ms");
}

#[test]
fn require_launch_timing_rejects_journal_total_without_phases() {
    let error = dashboard_check::require_launch_timing(RUNNING_BOX_CPU_NULL.as_bytes(), "smoke")
        .unwrap_err();
    assert!(matches!(
        error,
        CheckError::RequireLaunchTimingPhasesMissing
    ));
}

#[test]
fn launch_timing_log_line_lists_phase_fields() {
    let timing =
        dashboard_check::require_launch_timing(CAPTURE_HOST_DISK.as_bytes(), "running-box")
            .expect("timing");
    let line = dashboard_check::launch_timing_log_line("running-box", &timing);
    assert!(line.contains("total_ms=8420.7"));
    assert!(line.contains("storage_ms=4200.5"));
    assert!(line.contains("vmm_ms=1800.2"));
}

#[test]
fn require_launch_timing_rejects_missing_box_name() {
    let error =
        dashboard_check::require_launch_timing(RUNNING_BOX_LEGACY_LAUNCH.as_bytes(), "kiln-smoke")
            .unwrap_err();
    assert!(matches!(error, CheckError::RequireLaunchTimingBoxNotFound));
}

#[test]
fn rejects_null_host_disk() {
    let body = br#"{"schema_version":1,"version":"0.4.17","observed_at":"2026-10-21T12:00:01Z","workspace":"local","host":{"name":"local","state":"online","metrics":{"cpu_percent":1.0,"memory_bytes":5000,"disk_bytes":null,"disk_total_bytes":8000}},"boxes":[],"templates":[],"checkpoints":[],"operations":[]}"#;
    let error = dashboard_check::parse_and_validate(body).unwrap_err();
    assert!(matches!(error, CheckError::HostDiskMissing));
}
