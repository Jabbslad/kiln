//! Shared tenant-scoped JWT fixtures for integration tests (matches A1 token minting).

pub const FIXTURE_TENANT_ID: &str = "ten_test";

pub fn fixture_tenant_id() -> Option<String> {
    Some(FIXTURE_TENANT_ID.into())
}
