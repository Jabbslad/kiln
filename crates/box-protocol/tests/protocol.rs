use box_protocol::{
    ExecRequest, MAX_FRAME_SIZE, PROTOCOL_VERSION, Request, read_frame, write_frame,
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
