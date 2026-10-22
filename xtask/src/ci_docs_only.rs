use crate::register_cases;
use std::path::Path;
use std::process::Command;

const HEAVY_PREFIXES: &[&str] = &["deploy/", "scripts/", ".github/"];

const LOCKFILES: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lock",
    "bun.lockb",
    "poetry.lock",
    "uv.lock",
    "composer.lock",
    "Gemfile.lock",
    "go.sum",
];

const CODE_SUFFIXES: &[&str] = &[
    ".rs", ".py", ".sh", ".bash", ".js", ".mjs", ".cjs", ".ts", ".tsx", ".jsx", ".toml", ".yml",
    ".yaml", ".json",
];

pub fn normalize(path: &str) -> String {
    let mut rel = path.trim().replace('\\', "/");
    rel = rel.trim_start_matches('/').to_string();
    if let Some(stripped) = rel.strip_prefix("./") {
        rel = stripped.to_string();
    }
    rel
}

pub fn is_documentation(path: &str) -> bool {
    let rel = normalize(path);
    if rel.is_empty() || rel.ends_with('/') {
        return false;
    }
    if HEAVY_PREFIXES.iter().any(|prefix| rel.starts_with(prefix)) {
        return false;
    }
    let name = rel.rsplit('/').next().unwrap_or(rel.as_str());
    if name == "Cargo.toml" || LOCKFILES.contains(&name) || name.ends_with(".lock") {
        return false;
    }
    if name.ends_with(".md") {
        return true;
    }
    rel.starts_with("docs/") && !CODE_SUFFIXES.iter().any(|suffix| name.ends_with(suffix))
}

pub fn needs_deploy_final_checks(paths: &[String]) -> bool {
    let cleaned: Vec<String> = paths
        .iter()
        .filter(|path| !path.trim().is_empty())
        .map(|path| normalize(path))
        .collect();
    cleaned.iter().any(|path| {
        path.starts_with("deploy/") || path.starts_with("scripts/") || path.starts_with("crates/")
    })
}

pub fn needs_heavy(paths: &[String]) -> bool {
    let cleaned: Vec<String> = paths
        .iter()
        .filter(|path| !path.trim().is_empty())
        .map(|path| normalize(path))
        .collect();
    if cleaned.is_empty() {
        return true;
    }
    cleaned.iter().any(|path| !is_documentation(path))
}

pub fn changed_paths(base: &str, head: &str, cwd: Option<&Path>) -> Result<Vec<String>, String> {
    let mut command = Command::new("git");
    command.args(["diff", "--name-only", "--no-renames", base, head]);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command
        .output()
        .map_err(|error| format!("git diff failed to start: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git diff failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(ToString::to_string)
        .collect())
}

pub fn classify(paths: &[String]) -> String {
    format!(
        "heavy={}\ndeploy_final={}\n",
        if needs_heavy(paths) { "true" } else { "false" },
        if needs_deploy_final_checks(paths) {
            "true"
        } else {
            "false"
        }
    )
}

pub fn cli(args: &[String]) -> i32 {
    let paths = if args.len() == 2 {
        match changed_paths(&args[0], &args[1], None) {
            Ok(paths) => paths,
            Err(error) => {
                eprintln!("{error}");
                return 1;
            }
        }
    } else {
        let mut input = String::new();
        let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut input);
        input.lines().map(ToString::to_string).collect()
    };
    print!("{}", classify(&paths));
    0
}

fn test_runbooks_and_markdown_skip_the_compile_jobs() {
    let paths = vec![
        "docs/runbooks/2026-10-19-public-door.md".to_string(),
        "docs/releases.md".to_string(),
        "README.md".to_string(),
        "web/README.md".to_string(),
        "AGENTS.md".to_string(),
        "docs/picture.png".to_string(),
    ];
    assert!(!needs_heavy(&paths));
    assert_eq!(classify(&paths), "heavy=false\ndeploy_final=false\n");
}

fn test_code_scripts_workflows_deploy_and_locks_build() {
    let samples = [
        "crates/kiln-server/src/main.rs",
        "scripts/test-bootstrap.py",
        "scripts/README.md",
        ".github/workflows/build.yml",
        "deploy/bootstrap/README.md",
        "deploy/install.sh",
        "Cargo.toml",
        "crates/kiln-api/Cargo.toml",
        "Cargo.lock",
        "web/package-lock.json",
        "web/package.json",
        "images/build-ubuntu.sh",
        "docs/notes.rs",
        "docs/tool.py",
        "docs/extra.json",
    ];
    for path in samples {
        let with_doc = vec![path.to_string(), "docs/releases.md".to_string()];
        assert!(needs_heavy(&with_doc), "{path}");
        let deploy_final = if path.starts_with(".github/") {
            "false"
        } else {
            "true"
        };
        assert_eq!(
            classify(&[path.to_string()]),
            format!("heavy=true\ndeploy_final={deploy_final}\n"),
            "{path}"
        );
    }
}

fn test_empty_diff_builds() {
    assert!(needs_heavy(&[]));
    assert!(needs_heavy(&[String::new(), "  ".to_string()]));
    assert_eq!(classify(&[]), "heavy=true\ndeploy_final=false\n");
}

fn test_deploy_final_checks_only_for_deploy_scripts_crates() {
    assert!(!needs_deploy_final_checks(&["README.md".to_string()]));
    assert!(!needs_deploy_final_checks(
        &["docs/releases.md".to_string()]
    ));
    assert!(!needs_deploy_final_checks(&["web/src/App.tsx".to_string()]));
    assert!(needs_deploy_final_checks(&[
        "crates/kiln-server/src/main.rs".to_string()
    ]));
    assert!(needs_deploy_final_checks(&["scripts/check.sh".to_string()]));
    assert!(needs_deploy_final_checks(
        &["deploy/install.sh".to_string()]
    ));
    assert!(!needs_deploy_final_checks(&[
        ".github/workflows/build.yml".to_string()
    ]));
}

fn test_rename_out_of_rust_lists_both_sides() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    git(root, &["init"]);
    git(root, &["config", "user.email", "test@example.com"]);
    git(root, &["config", "user.name", "test"]);
    let source = root.join("crates/kiln-server/src");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("main.rs"), "fn main() {}\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "add rust"]);
    let base = git(root, &["rev-parse", "HEAD"]);
    std::fs::create_dir_all(root.join("docs/runbooks")).unwrap();
    git(
        root,
        &[
            "mv",
            "crates/kiln-server/src/main.rs",
            "docs/runbooks/retired.md",
        ],
    );
    git(root, &["commit", "-m", "move rust under docs"]);
    let head = git(root, &["rev-parse", "HEAD"]);
    let paths = changed_paths(base.trim(), head.trim(), Some(root)).unwrap();
    assert!(
        paths
            .iter()
            .any(|path| path == "crates/kiln-server/src/main.rs")
    );
    assert!(paths.iter().any(|path| path == "docs/runbooks/retired.md"));
    assert!(needs_heavy(&paths));
    assert!(!needs_heavy(&["docs/runbooks/retired.md".to_string()]));
    let detected = git(root, &["diff", "--name-only", base.trim(), head.trim()]);
    assert!(
        !detected
            .lines()
            .any(|line| line == "crates/kiln-server/src/main.rs")
    );
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("git {args:?}: {error}"));
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

register_cases! {
    test_runbooks_and_markdown_skip_the_compile_jobs,
    test_code_scripts_workflows_deploy_and_locks_build,
    test_empty_diff_builds,
    test_rename_out_of_rust_lists_both_sides,
    test_deploy_final_checks_only_for_deploy_scripts_crates,
}
