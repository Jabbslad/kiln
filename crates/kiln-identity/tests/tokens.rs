use kiln_identity::tokens::Signer;
use p256::pkcs8::EncodePrivateKey;

#[test]
fn rotation_publishes_only_distinct_public_keys() {
    let key = p256::SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
    let pem = key.to_pkcs8_pem(Default::default()).unwrap();
    let active = Signer::from_pem("active", pem.as_bytes()).unwrap();
    let retiring = Signer::from_pem("retiring", pem.as_bytes()).unwrap().jwks();
    assert_eq!(
        active
            .clone()
            .with_retiring(retiring.clone())
            .unwrap()
            .jwks()["keys"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(active.clone().with_retiring(active.jwks()).is_err());
    let mut private = retiring.clone();
    private["keys"][0]["d"] = "not-public".into();
    assert!(active.clone().with_retiring(private).is_err());
    let mut duplicates = retiring.clone();
    duplicates["keys"]
        .as_array_mut()
        .unwrap()
        .push(retiring["keys"][0].clone());
    assert!(active.with_retiring(duplicates).is_err());
}
