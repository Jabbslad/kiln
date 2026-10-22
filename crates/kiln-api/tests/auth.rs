use kiln_api::auth::{RevocationQuery, RevocationSet, Scope, ServerClaims};
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
        "aud": "kiln:server:server-b", "tenant_id": "ten-a",
        "iat": 1900000000_u64, "nbf": 1900000000_u64, "exp": 1900000300_u64,
        "credential_id": "device-c", "scope": "read", "role": "owner"
    });
    let claims: ServerClaims = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(claims.aud, "kiln:server:server-b");
    assert_eq!(claims.scope, Scope::Read);
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

#[test]
fn revocation_wire_types_round_trip() {
    let query = RevocationQuery {
        tenant_id: "ten_a".into(),
    };
    let set = RevocationSet {
        accounts: vec!["acct_1".into()],
        credentials: vec!["dev_1".into()],
    };
    let query_back: RevocationQuery =
        serde_json::from_value(serde_json::to_value(&query).unwrap()).unwrap();
    assert_eq!(query_back.tenant_id, "ten_a");
    let set_back: RevocationSet =
        serde_json::from_value(serde_json::to_value(&set).unwrap()).unwrap();
    assert_eq!(set_back, set);
}
