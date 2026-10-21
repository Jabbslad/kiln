fn main() {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_default();
    let rest: Vec<String> = args.collect();
    let code = match command.as_str() {
        "ci-docs-only" => xtask::ci_docs_only::cli(&rest),
        "test-ci-docs-only" => xtask::ci_docs_only::cli_tests(),
        "merge-queue-tested" => xtask::merge_queue::cli(&rest),
        "test-merge-queue-tested" => xtask::merge_queue::cli_tests(),
        "required-gate" => xtask::required_gate::cli(&rest),
        "release" => xtask::release::cli(&rest),
        "test-release" => xtask::release::cli_tests(),
        "release-version" => xtask::release_version::cli(&rest),
        "test-release-version" => xtask::release_version::cli_tests(),
        "reuse-release" => xtask::reuse_release::cli(&rest),
        "test-reuse-release" => xtask::reuse_release::cli_tests(),
        "test-repo-guard" => xtask::repo_guard::cli_tests(),
        "central-login" => xtask::central_login::cli(&rest),
        #[cfg(unix)]
        "test-bootstrap" => xtask::bootstrap_tests::cli(&rest),
        #[cfg(unix)]
        "test-install" => xtask::install_tests::cli(&rest),
        #[cfg(unix)]
        "test-caddy-door" => xtask::caddy_door_tests::cli(&rest),
        #[cfg(unix)]
        "test-close-public-door" => xtask::close_public_door_tests::cli(&rest),
        #[cfg(unix)]
        "test-deploy-workflow" => xtask::deploy_workflow_tests::cli(&rest),
        #[cfg(unix)]
        "ssh-pty" => xtask::ssh_pty::cli(&rest),
        #[cfg(unix)]
        "warm-guest" => xtask::warm_guest::cli(&rest),
        #[cfg(not(unix))]
        "test-bootstrap"
        | "test-install"
        | "test-caddy-door"
        | "test-close-public-door"
        | "test-deploy-workflow"
        | "ssh-pty"
        | "warm-guest" => {
            eprintln!("{command} runs on unix");
            1
        }
        other => {
            eprintln!("unknown xtask command {other}");
            2
        }
    };
    std::process::exit(code);
}
