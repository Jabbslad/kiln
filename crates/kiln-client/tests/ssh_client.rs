#[tokio::test]
async fn ssh_methods_validate_inputs_before_networking() {
    let client = kiln_client::Client::new("https://localhost:1", &"a".repeat(64), None).unwrap();
    assert!(
        client
            .ssh_host_key("short")
            .await
            .unwrap_err()
            .to_string()
            .contains("invalid box ID")
    );
    assert!(
        client
            .ssh_tunnel(&"b".repeat(32), "ssh-rsa AAAA")
            .await
            .unwrap_err()
            .to_string()
            .contains("invalid SSH public key")
    );
}
