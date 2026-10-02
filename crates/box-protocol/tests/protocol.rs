use box_protocol::{
    ExecRequest, MAX_FRAME_SIZE, PROTOCOL_VERSION, Request, Response, SSH_VSOCK_PORT, SshConnect,
    SshReady, read_frame, valid_ssh_public_key, write_frame,
};

#[tokio::test]
async fn frames_round_trip_over_a_tokio_unix_stream() {
    let (mut a, mut b) = tokio::net::UnixStream::pair().unwrap();
    let request = Request::Exec(ExecRequest {
        argv: vec!["echo".into(), "hello".into()],
        cwd: Some("/tmp".into()),
        env: [("A".into(), "B".into())].into(),
        timeout_ms: 1000,
    });
    write_frame(&mut a, &request).await.unwrap();
    assert_eq!(read_frame::<_, Request>(&mut b).await.unwrap(), request);
}

#[tokio::test]
async fn oversized_frame_is_rejected_before_payload_read() {
    use tokio::io::AsyncWriteExt;
    let (mut a, mut b) = tokio::net::UnixStream::pair().unwrap();
    a.write_all(&((MAX_FRAME_SIZE + 1) as u32).to_be_bytes())
        .await
        .unwrap();
    assert!(matches!(
        read_frame::<_, Request>(&mut b).await,
        Err(box_protocol::FrameError::TooLarge { .. })
    ));
}

#[test]
fn messages_are_versioned_and_reject_unknown_fields() {
    let hello = format!(r#"{{"type":"hello","version":{PROTOCOL_VERSION},"extra":true}}"#);
    assert!(serde_json::from_str::<Request>(&hello).is_err());
}

#[test]
fn ssh_protocol_has_fixed_port_and_exact_wire_shapes() {
    assert_eq!(SSH_VSOCK_PORT, 1025);
    assert_eq!(
        serde_json::to_string(&Request::SshHostKey).unwrap(),
        r#"{"type":"ssh_host_key"}"#
    );
    assert_eq!(
        serde_json::to_string(&Response::SshHostKey {
            public_key: "key".into()
        })
        .unwrap(),
        r#"{"type":"ssh_host_key","public_key":"key"}"#
    );
    assert!(serde_json::from_str::<SshConnect>(r#"{"public_key":"key","extra":1}"#).is_err());
    assert!(serde_json::from_str::<SshReady>(r#"{"public_key":"key","extra":1}"#).is_err());
}

#[test]
fn only_canonical_comment_free_ed25519_keys_are_valid() {
    let key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    assert!(valid_ssh_public_key(key));
    for invalid in [
        "",
        "ssh-rsa AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA comment",
        "from=\"*\" ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n",
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE4AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    ] {
        assert!(!valid_ssh_public_key(invalid), "accepted {invalid:?}");
    }
}
