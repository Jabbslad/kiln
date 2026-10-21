use crate::register_cases;
use crate::repo_root;
use std::fs;
use std::path::Path;

fn test_repository_has_no_python_files() {
    let mut found = Vec::new();
    walk(&repo_root(), &mut found);
    assert!(found.is_empty(), "python files remain: {found:?}");
}

fn test_workflows_do_not_set_up_python() {
    let workflows = repo_root().join(".github/workflows");
    for entry in fs::read_dir(&workflows).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("yml") {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("setup-python"),
            "{} still sets up python",
            path.display()
        );
        assert!(
            !text.contains("python ") && !text.contains("python3"),
            "{} still runs python",
            path.display()
        );
    }
}

fn test_proof_status_is_retired() {
    let root = repo_root();
    assert!(!root.join(".github/workflows/proof.yml").exists());
    assert!(!root.join("scripts/proof_status.py").exists());
    assert!(!root.join("scripts/test-proof-status.py").exists());
    let workflows = root.join(".github/workflows");
    for entry in fs::read_dir(&workflows).unwrap() {
        let path = entry.unwrap().path();
        let text = fs::read_to_string(&path).unwrap_or_default();
        assert!(!text.contains("proof_status"), "{}", path.display());
        assert!(!text.contains("context: proof"), "{}", path.display());
    }
}

fn walk(dir: &Path, found: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if name == ".git" || name == "target" || name == "node_modules" {
            continue;
        }
        if path.is_dir() {
            walk(&path, found);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("py") {
            found.push(path.display().to_string());
        }
    }
}

register_cases! {
    test_repository_has_no_python_files,
    test_workflows_do_not_set_up_python,
    test_proof_status_is_retired,
}
