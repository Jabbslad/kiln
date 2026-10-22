fn main() {
    // Cargo sets this only when the feature is on. Default builds leave it unset.
    let feature = std::env::var_os("CARGO_FEATURE_LOOPBACK_IDENTITY_CA").is_some();
    let release = std::env::var("PROFILE").ok().as_deref() == Some("release");
    if feature && release {
        panic!("loopback-identity-ca cannot be compiled into a release build");
    }
}
