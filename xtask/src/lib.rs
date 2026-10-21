use std::path::PathBuf;

pub mod central_login;
pub mod ci_docs_only;
pub mod merge_queue;
pub mod release;
pub mod release_version;
pub mod repo_guard;
pub mod required_gate;
pub mod reuse_release;

#[cfg(unix)]
pub mod bootstrap_tests;
#[cfg(unix)]
pub mod caddy_door_tests;
#[cfg(unix)]
pub mod close_public_door_tests;
#[cfg(unix)]
pub mod deploy_workflow_tests;
#[cfg(unix)]
pub mod install_tests;
#[cfg(unix)]
pub mod ssh_pty;
#[cfg(unix)]
pub mod warm_guest;

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("repository root")
}

pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else {
        "assertion failed".to_string()
    }
}

pub fn run_cases(cases: &[(&str, fn())]) -> i32 {
    let mut failed = 0usize;
    for (name, case) in cases {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(*case)) {
            Ok(()) => println!("ok {name}"),
            Err(payload) => {
                failed += 1;
                eprintln!("FAILED {name}: {}", panic_message(payload.as_ref()));
            }
        }
    }
    if failed == 0 { 0 } else { 1 }
}

#[macro_export]
macro_rules! register_cases {
    ($($name:ident),* $(,)?) => {
        pub fn cases() -> Vec<(&'static str, fn())> {
            vec![$((stringify!($name), $name)),*]
        }

        pub fn cli_tests() -> i32 {
            $crate::run_cases(&cases())
        }

        #[cfg(test)]
        mod case_harness {
            $(
                #[test]
                fn $name() {
                    super::$name();
                }
            )*
        }
    };
}
