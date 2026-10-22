use crate::ci_docs_only::{is_documentation, needs_heavy, normalize};

const RELEASE_PREFIXES: &[&str] = &["deploy/", "images/", "xtask/", "scripts/", ".github/"];

const RELEASE_ROOT: &[&str] = &["Cargo.toml", "Cargo.lock"];

pub fn is_release_path(path: &str) -> bool {
    let rel = normalize(path);
    if rel.is_empty() {
        return false;
    }
    if RELEASE_PREFIXES
        .iter()
        .any(|prefix| rel.starts_with(prefix))
    {
        return true;
    }
    if RELEASE_ROOT.contains(&rel.as_str()) {
        return true;
    }
    rel.ends_with("/Cargo.toml")
}

pub fn needs_web(paths: &[String]) -> bool {
    let cleaned: Vec<String> = paths
        .iter()
        .filter(|path| !path.trim().is_empty())
        .map(|path| normalize(path))
        .collect();
    if cleaned.is_empty() {
        return true;
    }
    cleaned
        .iter()
        .any(|path| path.starts_with("web/") || path.as_str() == "web")
}

pub fn needs_release_path(paths: &[String]) -> bool {
    let cleaned: Vec<String> = paths
        .iter()
        .filter(|path| !path.trim().is_empty())
        .map(|path| normalize(path))
        .collect();
    if cleaned.is_empty() {
        return true;
    }
    cleaned.iter().any(|path| is_release_path(path))
}

pub fn needs_docs_check(paths: &[String]) -> bool {
    let cleaned: Vec<String> = paths
        .iter()
        .filter(|path| !path.trim().is_empty())
        .map(|path| normalize(path))
        .collect();
    if cleaned.is_empty() {
        return false;
    }
    cleaned.iter().any(|path| is_documentation(path))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequestJobs {
    pub docs_check: bool,
    pub tag_path_dry_run: bool,
    pub server_check: bool,
    pub client: bool,
    pub dashboard: bool,
}

pub fn pull_request_jobs(paths: &[String]) -> PullRequestJobs {
    let heavy = needs_heavy(paths);
    PullRequestJobs {
        docs_check: needs_docs_check(paths),
        tag_path_dry_run: needs_release_path(paths),
        server_check: heavy,
        client: heavy,
        dashboard: heavy,
    }
}

pub fn classify(paths: &[String]) -> String {
    format!(
        "heavy={}\ndocs={}\nrelease_path={}\nweb={}\n",
        bool_line(needs_heavy(paths)),
        bool_line(needs_docs_check(paths)),
        bool_line(needs_release_path(paths)),
        bool_line(needs_web(paths)),
    )
}

fn bool_line(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

pub fn cli(args: &[String]) -> i32 {
    let paths = if args.len() == 2 {
        match crate::ci_docs_only::changed_paths(&args[0], &args[1], None) {
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

fn test_docs_only_pull_request_runs_docs_check_not_compile() {
    let paths = vec!["README.md".to_string(), "docs/releases.md".to_string()];
    let jobs = pull_request_jobs(&paths);
    assert_eq!(
        jobs,
        PullRequestJobs {
            docs_check: true,
            tag_path_dry_run: false,
            server_check: false,
            client: false,
            dashboard: false,
        }
    );
}

fn test_rust_only_pull_request_skips_docs_and_tag_path() {
    let paths = vec!["crates/kiln-server/src/lib.rs".to_string()];
    let jobs = pull_request_jobs(&paths);
    assert_eq!(
        jobs,
        PullRequestJobs {
            docs_check: false,
            tag_path_dry_run: false,
            server_check: true,
            client: true,
            dashboard: true,
        }
    );
    assert!(!needs_web(&paths));
}

fn test_web_paths_enable_dashboard_npm() {
    let paths = vec!["web/src/App.tsx".to_string()];
    assert!(needs_web(&paths));
    assert_eq!(
        classify(&paths),
        "heavy=true\ndocs=false\nrelease_path=false\nweb=true\n"
    );
}

fn test_deploy_change_runs_tag_path_not_docs() {
    let paths = vec!["deploy/install.sh".to_string()];
    let jobs = pull_request_jobs(&paths);
    assert_eq!(
        jobs,
        PullRequestJobs {
            docs_check: false,
            tag_path_dry_run: true,
            server_check: true,
            client: true,
            dashboard: true,
        }
    );
}

fn test_release_workflow_change_runs_tag_path() {
    let paths = vec![".github/workflows/build.yml".to_string()];
    let jobs = pull_request_jobs(&paths);
    assert!(jobs.tag_path_dry_run);
    assert!(jobs.server_check);
}

fn test_cargo_toml_bump_runs_tag_path() {
    let paths = vec!["Cargo.toml".to_string()];
    let jobs = pull_request_jobs(&paths);
    assert!(jobs.tag_path_dry_run);
    assert!(jobs.server_check);
}

fn test_docs_and_deploy_run_both_guards() {
    let paths = vec![
        "docs/note.md".to_string(),
        "scripts/plan-release.sh".to_string(),
    ];
    let jobs = pull_request_jobs(&paths);
    assert!(jobs.docs_check);
    assert!(jobs.tag_path_dry_run);
    assert!(jobs.server_check);
}

fn test_classify_emits_four_outputs() {
    let paths = vec!["README.md".to_string()];
    assert_eq!(
        classify(&paths),
        "heavy=false\ndocs=true\nrelease_path=false\nweb=false\n"
    );
}

crate::register_cases! {
    test_docs_only_pull_request_runs_docs_check_not_compile,
    test_rust_only_pull_request_skips_docs_and_tag_path,
    test_deploy_change_runs_tag_path_not_docs,
    test_release_workflow_change_runs_tag_path,
    test_cargo_toml_bump_runs_tag_path,
    test_docs_and_deploy_run_both_guards,
    test_web_paths_enable_dashboard_npm,
    test_classify_emits_four_outputs,
}
