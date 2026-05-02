use anyhow::{Context, Result};
use claudulhu_k8s_ops::k8s;
use data_encoding::BASE32_NOPAD;

pub async fn run(api_key: &str, gh_token: Option<&str>, noise_port: u16) -> Result<()> {
    // Generate a fresh Curve25519 keypair for rulyeh.
    let builder = snow::Builder::new(
        "Noise_XX_25519_ChaChaPoly_SHA256".parse().context("parse noise params")?,
    );
    let keypair = builder.generate_keypair().context("generate keypair")?;
    let mut combined = keypair.private.clone();
    combined.extend_from_slice(&keypair.public);
    let noise_private_key_hex = hex::encode(&combined);
    let pubkey_b32 = BASE32_NOPAD.encode(&keypair.public);

    let client = k8s::build_client().await?;

    println!("→ namespace");
    k8s::ensure_namespace(&client).await?;
    println!("→ RBAC");
    k8s::ensure_rbac(&client).await?;
    println!("→ secrets");
    k8s::upsert_secret(&client, api_key, gh_token, &noise_private_key_hex).await?;
    println!("→ PVC");
    k8s::ensure_rulyeh_pvc(&client).await?;
    println!("→ deployment");
    k8s::upsert_rulyeh_deployment(&client, noise_port).await?;
    println!("→ services");
    k8s::ensure_rulyeh_services(&client, noise_port).await?;
    println!("→ waiting for rulyeh to be ready...");
    k8s::wait_for_deployment_ready(&client, "rulyeh", 180).await?;

    let ip = k8s::get_node_external_ip(&client).await?;
    let qr_data = format!("2:{ip}:{noise_port}:{pubkey_b32}");

    println!("\nrulyeh is live at {ip}:{noise_port}");
    println!("QR data: {qr_data}\n");

    let code = qrcode::QrCode::new(&qr_data).context("generate QR code")?;
    let image = code
        .render::<qrcode::render::unicode::Dense1x2>()
        .dark_color(qrcode::render::unicode::Dense1x2::Dark)
        .light_color(qrcode::render::unicode::Dense1x2::Light)
        .build();
    println!("{image}");

    Ok(())
}
