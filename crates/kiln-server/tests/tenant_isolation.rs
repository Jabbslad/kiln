//! Tenant-scoped multi-account isolation on a disposable server harness
//! (`scripts/test-public-door.sh` pattern). Never targets dark-forge.

#[test]
#[ignore = "tenant stack (PRs 1/6–5/6) not merged"]
fn members_in_one_tenant_cannot_touch_each_others_boxes() {
    // Tenant T with owner and member-a, member-b (roles owner | member only).
    // Owner creates owner boxes; member-a creates member-a-x; member-b creates member-b-x.
    // member-a must not list or operate owner or member-b boxes (404 by name and id).
    // member-b symmetric. Door SSH and app terminal refuse cross-member and member→owner paths.
    // Each member fully operates their own boxes.
    panic!("wire disposable harness when the tenant stack merges");
}

#[test]
#[ignore = "tenant stack (PRs 1/6–5/6) not merged"]
fn tenant_b_member_is_refused_on_tenant_a_server() {
    // Two tenants A and B with separate enrolled servers.
    // A valid token for tenant B must not authenticate on tenant A's server (401).
    panic!("wire disposable harness when the tenant stack merges");
}
