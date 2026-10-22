use kiln_api::{Caller, Operation, OperationState, may_access_box};
use kiln_runtime::launch_timing::LaunchTiming;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;

#[derive(Default)]
pub(crate) struct Inventory {
    pub boxes: Vec<BoxFacts>,
    pub templates: Vec<TemplateFacts>,
    pub operations: Vec<OperationFacts>,
    pub checkpoints: Vec<CheckpointFacts>,
}

pub(crate) struct CheckpointFacts {
    pub id: String,
    pub box_id: String,
    pub created_at: i64,
    pub size_bytes: Option<u64>,
    pub compatible: bool,
}

pub(crate) struct BoxFacts {
    pub id: String,
    pub name: String,
    pub owner: Option<String>,
    pub state: String,
    pub memory_mib: u32,
    pub vcpus: u8,
    pub disk_bytes: Option<u64>,
    pub created_at: i64,
    pub last_operation: Option<String>,
    pub last_error: Option<String>,
    pub restored_from: Option<String>,
    pub metrics: crate::metrics::Snapshot,
    pub timing: Option<LaunchTiming>,
}

pub(crate) struct TemplateFacts {
    pub id: String,
    pub name: String,
    pub memory_mib: u32,
    pub vcpus: u8,
    pub development_only: bool,
    pub disk_bytes: Option<u64>,
}

pub(crate) struct OperationFacts {
    pub id: String,
    pub box_id: String,
    pub box_name: String,
    pub is_create: bool,
    pub created_at: i64,
    pub operation: Operation,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Document {
    schema_version: u32,
    version: String,
    observed_at: String,
    workspace: String,
    host: HostBody,
    boxes: Vec<BoxBody>,
    templates: Vec<TemplateBody>,
    checkpoints: Vec<CheckpointBody>,
    operations: Vec<OperationBody>,
}

#[derive(Serialize, Deserialize)]
struct HostBody {
    name: String,
    state: String,
    metrics: crate::metrics::Snapshot,
}

#[derive(Serialize, Deserialize)]
struct Resources {
    vcpus: u32,
    memory_bytes: u64,
    disk_bytes: u64,
}

#[derive(Serialize, Deserialize)]
struct BoxBody {
    id: String,
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owner: Option<String>,
    state: String,
    image: String,
    resources: Option<Resources>,
    metrics: crate::metrics::Snapshot,
    created_at: String,
    last_operation: Option<String>,
    last_error: Option<String>,
    restored_from: Option<String>,
    timing: Option<LaunchTiming>,
}

#[derive(Serialize, Deserialize)]
struct TemplateBody {
    id: String,
    name: String,
    version: String,
    description: String,
    compatible: bool,
    development_only: bool,
    resources: Resources,
}

#[derive(Serialize, Deserialize)]
struct CheckpointBody {
    id: String,
    box_id: String,
    name: String,
    created_at: String,
    size_bytes: Option<u64>,
    compatible: bool,
}

#[derive(Serialize, Deserialize)]
struct OperationBody {
    id: String,
    box_id: String,
    box_name: String,
    kind: String,
    state: String,
    created_at: String,
    detail: String,
    steps: Vec<Value>,
}

pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

pub(crate) fn utc_timestamp(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let tod = u32::try_from(secs.rem_euclid(86_400)).unwrap_or(0);
    let (year, month, day) = civil_date(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60,
    )
}

const UNENROLLED_WORKSPACE: &str = "local";

fn published_workspace(name: &str) -> &str {
    if name.is_empty() {
        UNENROLLED_WORKSPACE
    } else {
        name
    }
}

impl Document {
    pub(crate) fn host_metrics(&self) -> crate::metrics::Snapshot {
        self.host.metrics
    }

    pub(crate) fn running_without_launch_timing(&self) -> bool {
        self.boxes.iter().any(|box_body| {
            box_body.state == "running"
                && box_body
                    .timing
                    .as_ref()
                    .is_some_and(|timing| timing.total_ms.is_none())
        })
    }

    pub(crate) fn named_box_timing(&self, identifier: &str) -> Option<Option<LaunchTiming>> {
        self.boxes
            .iter()
            .find(|box_body| box_body.id == identifier || box_body.name == identifier)
            .map(|box_body| box_body.timing.clone())
    }
}

pub(crate) fn document(
    workspace: &str,
    observed_at: &str,
    host_state: &'static str,
    host_metrics: crate::metrics::Snapshot,
    inventory: &Inventory,
) -> Document {
    let workspace = published_workspace(workspace).to_owned();
    let mut operations = Vec::new();
    for row in &inventory.operations {
        let Some(state) = operation_state(row.operation.state) else {
            continue;
        };
        operations.push(OperationBody {
            id: row.id.clone(),
            box_id: row.box_id.clone(),
            box_name: row.box_name.clone(),
            kind: if row.is_create {
                "create".to_string()
            } else {
                "unknown".to_string()
            },
            state: state.to_string(),
            created_at: utc_timestamp(row.created_at),
            detail: client_detail(&row.operation),
            steps: Vec::new(),
        });
    }
    Document {
        schema_version: 1,
        version: kiln_api::release_version().to_owned(),
        observed_at: observed_at.to_owned(),
        workspace: workspace.clone(),
        host: HostBody {
            name: workspace,
            state: host_state.to_string(),
            metrics: host_metrics,
        },
        boxes: inventory.boxes.iter().map(box_body).collect(),
        templates: inventory
            .templates
            .iter()
            .filter_map(template_body)
            .collect(),
        checkpoints: inventory.checkpoints.iter().map(checkpoint_body).collect(),
        operations,
    }
}

pub(crate) fn stamp_value(value: &mut Value, workspace: &str, observed_at: &str) {
    if !value.is_object() {
        return;
    }
    let workspace = published_workspace(workspace);
    value["version"] = json!(kiln_api::release_version());
    value["observed_at"] = json!(observed_at);
    value["workspace"] = json!(workspace);
    if let Some(host) = value.get_mut("host") {
        host["name"] = json!(workspace);
    }
}

pub(crate) fn scope_to_caller(
    mut document: Document,
    caller: &Caller,
    tenant_owner: &str,
) -> Document {
    let visible: HashSet<_> = document
        .boxes
        .iter()
        .filter(|box_body| may_access_box(caller, box_body.owner.as_deref(), tenant_owner))
        .map(|box_body| box_body.id.clone())
        .collect();
    document
        .boxes
        .retain(|box_body| visible.contains(&box_body.id));
    document
        .operations
        .retain(|operation| visible.contains(&operation.box_id));
    document
        .checkpoints
        .retain(|checkpoint| visible.contains(&checkpoint.box_id));
    document
}

fn box_body(facts: &BoxFacts) -> BoxBody {
    BoxBody {
        id: facts.id.clone(),
        name: facts.name.clone(),
        owner: facts.owner.clone(),
        state: box_state(&facts.state).to_string(),
        image: String::new(),
        resources: resources(facts.vcpus, facts.memory_mib, facts.disk_bytes),
        metrics: facts.metrics,
        created_at: utc_timestamp(facts.created_at),
        last_operation: facts.last_operation.clone(),
        last_error: public_error(facts.last_error.as_deref()),
        restored_from: facts.restored_from.clone(),
        timing: facts.timing.clone(),
    }
}

fn checkpoint_body(facts: &CheckpointFacts) -> CheckpointBody {
    CheckpointBody {
        id: facts.id.clone(),
        box_id: facts.box_id.clone(),
        name: facts.id.clone(),
        created_at: utc_timestamp(facts.created_at),
        size_bytes: facts.size_bytes,
        compatible: facts.compatible,
    }
}

fn template_body(facts: &TemplateFacts) -> Option<TemplateBody> {
    Some(TemplateBody {
        id: facts.id.clone(),
        name: facts.name.clone(),
        version: String::new(),
        description: String::new(),
        compatible: true,
        development_only: facts.development_only,
        resources: resources(facts.vcpus, facts.memory_mib, facts.disk_bytes)?,
    })
}

fn resources(vcpus: u8, memory_mib: u32, disk_bytes: Option<u64>) -> Option<Resources> {
    let disk_bytes = disk_bytes.filter(|bytes| *bytes > 0)?;
    if vcpus == 0 || memory_mib == 0 {
        return None;
    }
    Some(Resources {
        vcpus: u32::from(vcpus),
        memory_bytes: u64::from(memory_mib) * 1024 * 1024,
        disk_bytes,
    })
}

fn box_state(state: &str) -> &'static str {
    match state {
        "running" => "running",
        "paused" => "paused",
        "stopped" => "stopped",
        "failed" => "failed",
        "creating" => "creating",
        _ => "unknown",
    }
}

fn operation_state(state: OperationState) -> Option<&'static str> {
    match state {
        OperationState::Running => Some("running"),
        OperationState::Succeeded => Some("completed"),
        OperationState::Failed => Some("failed"),
        OperationState::Unknown => None,
    }
}

fn client_detail(operation: &Operation) -> String {
    public_error(operation.error.as_ref().map(|error| error.message.as_str())).unwrap_or_default()
}

fn public_error(value: Option<&str>) -> Option<String> {
    let value = value?;
    if value.contains('/') || value.contains('\\') {
        return None;
    }
    Some(value.to_owned())
}

fn civil_date(days_since_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = z.div_euclid(146_097);
    let doe = u64::try_from(z - era * 146_097).unwrap_or(0);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let year = i64::try_from(yoe).unwrap_or(0) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year + i64::from(month <= 2);
    (
        i32::try_from(year).unwrap_or(0),
        u32::try_from(month).unwrap_or(0),
        u32::try_from(day).unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_timestamps_match_known_instants() {
        assert_eq!(utc_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_timestamp(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(utc_timestamp(1_700_000_100), "2023-11-14T22:15:00Z");
    }
}
