use kiln_api::auth::{Role, Scope, ServerClaims};
use serde_json::json;

#[test]
fn scope_never_defaults_or_accepts_administrator() {
    assert_eq!(
        serde_json::from_str::<Scope>("\"read\"").unwrap(),
        Scope::Read
    );
    assert_eq!(
        serde_json::from_str::<Scope>("\"operate\"").unwrap(),
        Scope::Operate
    );
    for value in ["null", "\"admin\"", "\"\"", "1"] {
        assert!(serde_json::from_str::<Scope>(value).is_err());
    }
}

#[test]
fn server_claims_require_every_authority_and_time_field() {
    let value = json!({
        "iss": "https://identity.example.test", "sub": "owner-a",
        "aud": "kiln:server:server-b", "tenant_id": "tenant-t",
        "iat": 1900000000_u64, "nbf": 1900000000_u64, "exp": 1900000300_u64,
        "credential_id": "device-c", "scope": "read", "role": "owner"
    });
    let claims: ServerClaims = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(claims.aud, "kiln:server:server-b");
    assert_eq!(claims.scope, Scope::Read);
    assert_eq!(claims.role, Some(Role::Owner));
    assert_eq!(claims.tenant_id.as_deref(), Some("tenant-t"));
    for field in [
        "iss",
        "sub",
        "aud",
        "iat",
        "nbf",
        "exp",
        "credential_id",
        "scope",
    ] {
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<ServerClaims>(missing).is_err(),
            "{field}"
        );
    }
    let legacy = json!({
        "iss": "https://identity.example.test", "sub": "owner-a",
        "aud": "kiln:server:server-b",
        "iat": 1900000000_u64, "nbf": 1900000000_u64, "exp": 1900000300_u64,
        "credential_id": "device-c", "scope": "read"
    });
    let parsed: ServerClaims = serde_json::from_value(legacy).unwrap();
    assert!(parsed.tenant_id.is_none());
    assert!(parsed.role.is_none());
    let mut unknown = value;
    unknown["admin"] = json!(true);
    assert!(serde_json::from_value::<ServerClaims>(unknown).is_err());
}
