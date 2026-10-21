use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LaunchTiming {
    pub method: String,
    pub cache: String,
    pub disk_copy: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vmm_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub init_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_command_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_ms: Option<f64>,
}

impl LaunchTiming {
    pub fn from_phases(
        method: &str,
        disk_copy: &str,
        total_ms: f64,
        phases: BTreeMap<&'static str, f64>,
    ) -> Self {
        let sum = |keys: &[&str]| {
            let total: f64 = keys
                .iter()
                .filter_map(|key| phases.get(*key))
                .copied()
                .sum();
            (total > 0.).then_some(total)
        };
        Self {
            method: method.into(),
            cache: "uncontrolled".into(),
            disk_copy: disk_copy.into(),
            storage_ms: sum(&[
                "image_verification",
                "allocation_wait",
                "reservation",
                "disk_copy",
                "snapshot_verification",
            ]),
            vmm_ms: sum(&["boot_preflight", "vmm_start", "vmm_configure"]),
            restore_ms: phases.get("snapshot_verification").copied(),
            agent_ms: sum(&["guest_initialize"]),
            init_ms: phases.get("guest_ready").copied(),
            first_command_ms: None,
            total_ms: Some(total_ms),
        }
    }

    pub fn from_journal(method: &str, disk_copy: &str, total_ms: f64) -> Self {
        Self {
            method: method.into(),
            cache: "unmeasured".into(),
            disk_copy: disk_copy.into(),
            storage_ms: None,
            vmm_ms: None,
            restore_ms: None,
            agent_ms: None,
            init_ms: None,
            first_command_ms: None,
            total_ms: Some(total_ms),
        }
    }

    pub fn reported(&self) -> bool {
        self.total_ms.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phases_map_to_dashboard_fields() {
        let mut phases = BTreeMap::new();
        phases.insert("disk_copy", 100.);
        phases.insert("vmm_start", 50.);
        phases.insert("guest_ready", 25.);
        let timing = LaunchTiming::from_phases("create", "Copy", 200., phases);
        assert_eq!(timing.storage_ms, Some(100.));
        assert_eq!(timing.vmm_ms, Some(50.));
        assert_eq!(timing.init_ms, Some(25.));
        assert_eq!(timing.total_ms, Some(200.));
    }
}
