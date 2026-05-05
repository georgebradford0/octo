use claudulhu_core::noise::{
    noise_handshake, read_noise_frame, write_noise_frame, DEV_STATIC_PRIVATE, DEV_STATIC_PUBLIC,
    NOISE_PATTERN,
};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

/// Perform the Noise XX handshake as the initiator (client side).
/// Returns the transport state and the server's received static public key.
/// The caller is responsible for verifying the static key matches the QR code value.
async fn client_handshake(
    stream: &mut TcpStream,
) -> anyhow::Result<(snow::TransportState, Vec<u8>)> {
    let kp = snow::Builder::new(NOISE_PATTERN.parse()?)
        .generate_keypair()?;
    let mut hs = snow::Builder::new(NOISE_PATTERN.parse()?)
        .local_private_key(&kp.private)
        .build_initiator()?;

    let mut buf = vec![0u8; 65535];

    // msg1 → server
    let n = hs.write_message(&[], &mut buf)?;
    stream.write_all(&(n as u16).to_be_bytes()).await?;
    stream.write_all(&buf[..n]).await?;

    // msg2 ← server (contains server's static public key)
    let msg2 = read_noise_frame(stream).await?;
    hs.read_message(&msg2, &mut buf)?;

    // msg3 → server
    let n = hs.write_message(&[], &mut buf)?;
    write_noise_frame(stream, &buf[..n]).await?;

    let server_static = hs.get_remote_static()
        .ok_or_else(|| anyhow::anyhow!("no remote static key after handshake"))?
        .to_vec();

    Ok((hs.into_transport_mode()?, server_static))
}

/// Verify that the Noise XX handshake completes and the resulting session
/// keys produce a working encrypted channel.
#[tokio::test]
async fn noise_handshake_completes() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server_task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        noise_handshake(&mut stream, &DEV_STATIC_PRIVATE)
            .await
            .expect("server handshake failed")
    });

    let mut client_stream = TcpStream::connect(addr).await.unwrap();
    let (mut client_ts, _) = client_handshake(&mut client_stream)
        .await
        .expect("client handshake failed");

    let mut server_ts = server_task.await.unwrap();

    // Verify the shared session works: client encrypts, server decrypts.
    let plaintext = b"hello rulyeh";
    let mut ciphertext = vec![0u8; plaintext.len() + 64];
    let enc_len = client_ts.write_message(plaintext, &mut ciphertext).unwrap();

    let mut decrypted = vec![0u8; plaintext.len() + 64];
    let dec_len = server_ts
        .read_message(&ciphertext[..enc_len], &mut decrypted)
        .unwrap();

    assert_eq!(&decrypted[..dec_len], plaintext);

    // And the reverse: server encrypts, client decrypts.
    let reply = b"hello client";
    let mut ciphertext2 = vec![0u8; reply.len() + 64];
    let enc_len2 = server_ts.write_message(reply, &mut ciphertext2).unwrap();

    let mut decrypted2 = vec![0u8; reply.len() + 64];
    let dec_len2 = client_ts
        .read_message(&ciphertext2[..enc_len2], &mut decrypted2)
        .unwrap();

    assert_eq!(&decrypted2[..dec_len2], reply);
}

/// Verify that the client can read the server's static public key after the
/// handshake. This is how QR-code authentication works: the client compares
/// the received key against the one encoded in the QR code and rejects the
/// connection if they differ. Noise XX completes at the protocol level
/// regardless — rejection is the application's responsibility.
#[tokio::test]
async fn noise_handshake_exposes_server_static_key() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = noise_handshake(&mut stream, &DEV_STATIC_PRIVATE).await;
    });

    let mut stream = TcpStream::connect(addr).await.unwrap();
    let (_, received_key) = client_handshake(&mut stream).await.unwrap();

    // Client can verify this against the key from the QR code.
    assert_eq!(received_key, DEV_STATIC_PUBLIC);

    // Connecting to a server with a different key would yield a different
    // received_key, and the application should close the connection.
}
