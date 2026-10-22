use serde_json::Value;

const PASSING: &[&str] = &["success", "skipped"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blocker {
    pub job_id: String,
    pub result: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision {
    pub blockers: Vec<Blocker>,
}

impl Decision {
    pub fn ok(&self) -> bool {
        self.blockers.is_empty()
    }
}

pub fn evaluate(needs: &Value) -> Decision {
    let Some(map) = needs.as_object() else {
        return Decision {
            blockers: vec![Blocker {
                job_id: "needs".to_string(),
                result: "malformed".to_string(),
            }],
        };
    };
    if map.is_empty() {
        return Decision {
            blockers: vec![Blocker {
                job_id: "needs".to_string(),
                result: "empty".to_string(),
            }],
        };
    }
    let mut blockers = Vec::new();
    for (job_id, entry) in map {
        let result = result_of(entry);
        if PASSING.contains(&result.as_str()) {
            continue;
        }
        blockers.push(Blocker {
            job_id: job_id.clone(),
            result,
        });
    }
    Decision { blockers }
}

fn result_of(entry: &Value) -> String {
    let Some(map) = entry.as_object() else {
        return "malformed".to_string();
    };
    match map.get("result").and_then(Value::as_str) {
        Some(result) if !result.is_empty() => result.to_string(),
        _ => "malformed".to_string(),
    }
}

pub fn run_gate(raw: Option<&str>) -> (i32, String) {
    let raw = raw.unwrap_or("");
    if raw.trim().is_empty() {
        return (1, "needs malformed\n".to_string());
    }
    let payload = match serde_json::from_str::<Value>(raw) {
        Ok(payload) => payload,
        Err(_) => return (1, "needs malformed\n".to_string()),
    };
    let decision = evaluate(&payload);
    if decision.ok() {
        return (0, String::new());
    }
    let mut stdout = String::new();
    for blocker in &decision.blockers {
        stdout.push_str(&format!("{} {}\n", blocker.job_id, blocker.result));
    }
    (1, stdout)
}

pub fn cli(_args: &[String]) -> i32 {
    let raw = std::env::var("NEEDS_JSON").ok();
    let (code, stdout) = run_gate(raw.as_deref());
    print!("{stdout}");
    code
}

pub fn should_cancel_workflow(raw: Option<&str>) -> bool {
    let raw = raw.unwrap_or("");
    if raw.trim().is_empty() {
        return false;
    }
    let payload = match serde_json::from_str::<Value>(raw) {
        Ok(payload) => payload,
        Err(_) => return false,
    };
    !evaluate(&payload).ok()
}

pub fn cli_cancel(_args: &[String]) -> i32 {
    if should_cancel_workflow(std::env::var("NEEDS_JSON").ok().as_deref()) {
        0
    } else {
        1
    }
}

fn test_should_cancel_when_a_need_failed() {
    let payload = serde_json::json!({
        "server-check": {"result": "failure"},
        "client": {"result": "success"},
    });
    assert!(should_cancel_workflow(Some(&payload.to_string())));
}

fn test_should_not_cancel_when_all_needs_pass_or_skip() {
    let payload = serde_json::json!({
        "docs-check": {"result": "skipped"},
        "server-check": {"result": "success"},
    });
    assert!(!should_cancel_workflow(Some(&payload.to_string())));
}

crate::register_cases! {
    test_should_cancel_when_a_need_failed,
    test_should_not_cancel_when_all_needs_pass_or_skip,
}
