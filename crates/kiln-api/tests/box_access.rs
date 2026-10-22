use kiln_api::auth::{Caller, Role, caller_may_access_box, filter_visible_boxes};

#[test]
fn owner_role_sees_every_box() {
    let owner = Caller {
        sub: "acct-owner".into(),
        tenant_id: "ten_1".into(),
        role: Role::Owner,
    };
    assert!(caller_may_access_box(&owner, "acct-owner"));
    assert!(caller_may_access_box(&owner, "acct-other"));
}

#[test]
fn member_role_only_sees_own_boxes() {
    let member = Caller {
        sub: "acct-b".into(),
        tenant_id: "ten_1".into(),
        role: Role::Member,
    };
    assert!(caller_may_access_box(&member, "acct-b"));
    assert!(!caller_may_access_box(&member, "acct-a"));
}

#[test]
fn list_filter_matches_access_rules() {
    let owners = ["acct-a", "acct-b", "acct-a"];
    let owner = Caller {
        sub: "acct-owner".into(),
        tenant_id: "ten_1".into(),
        role: Role::Owner,
    };
    assert_eq!(filter_visible_boxes(&owner, owners).count(), 3);

    let member = Caller {
        sub: "acct-b".into(),
        tenant_id: "ten_1".into(),
        role: Role::Member,
    };
    let visible: Vec<_> = filter_visible_boxes(&member, owners).collect();
    assert_eq!(visible, vec!["acct-b"]);
}
