//! Relay-signing keypair + best-effort `/notify` client.
//!
//! Lair (and any other server-side caller) holds an Ed25519 keypair separate
//! from its Noise X25519 keypair. The Ed25519 public key is published to
//! mobile via the lair `/info` endpoint over the encrypted Noise tunnel, so
//! the public key never has to round-trip through the relay before mobile
//! trusts it. The private key signs `/notify` POST bodies the relay then
//! forwards to APNs/FCM.
//!
//! Keys are persisted as the raw 32-byte seed (Ed25519 SecretKey input) at a
//! caller-specified path. Same on-disk shape lair already uses for the Noise
//! key — single read/write, no PEM wrappers.

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use ed25519_dalek::{Signer, SigningKey};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

/// Wraps an Ed25519 signing key with helpers to expose its public half in
/// the same RFC4648 base32 (no padding) shape mobile already uses for the
/// Noise pubkey, so both can sit side-by-side in QR codes and JSON.
pub struct RelaySigner {
    pub signing: SigningKey,
}

impl RelaySigner {
    /// Load the 32-byte seed from `path`, or generate + persist a new one.
    /// Mirrors `noise::load_or_generate_keypair`'s "drop a file in /data,
    /// reuse it forever" idiom.
    pub fn load_or_generate(path: &str) -> Self {
        if let Ok(bytes) = std::fs::read(path) {
            if bytes.len() == 32 {
                let seed: [u8; 32] = bytes.try_into().unwrap();
                info!("[relay] loaded existing signing key from {path}");
                return Self { signing: SigningKey::from_bytes(&seed) };
            }
            warn!("[relay] signing key at {path} has wrong length, regenerating");
        } else {
            info!("[relay] no signing key at {path}, generating new one");
        }
        let mut seed = [0u8; 32];
        OsRng.fill_bytes(&mut seed);
        let signing = SigningKey::from_bytes(&seed);
        if let Some(parent) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(path, seed).ok();
        info!("[relay] saved new signing key to {path}");
        Self { signing }
    }

    /// Public key in RFC4648 base32 (no padding). Same encoding as the Noise
    /// pubkey in `core::noise::to_base32`, so mobile can compare formats.
    pub fn pubkey_b32(&self) -> String {
        crate::noise::to_base32(self.signing.verifying_key().as_bytes())
    }
}

/// Body posted to the relay. The relay only verifies the Ed25519 signature
/// over the raw bytes of the JSON-serialised body — `category`, `title`, and
/// `body` are passed straight through to APNs/FCM (or, when both are absent,
/// the relay sends a silent push instead of an alert).
#[derive(Serialize)]
struct NotifyBody<'a> {
    ts:       i64,
    nonce:    String,
    category: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")] title: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")] body:  Option<&'a str>,
}

/// Best-effort POST. Logs and returns; never bubbles up to the caller — push
/// failures should not interrupt the agentic loop or background-task
/// completion path.
pub async fn notify(
    relay_url: &str,
    signer:    &RelaySigner,
    category:  &str,
    title:     Option<&str>,
    body:      Option<&str>,
) {
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let mut nonce_bytes = [0u8; 16];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = hex::encode(nonce_bytes);

    let payload = NotifyBody { ts, nonce, category, title, body };
    let raw = match serde_json::to_vec(&payload) {
        Ok(v) => v,
        Err(e) => {
            warn!("[relay] serialise notify body: {e}");
            return;
        }
    };
    let sig = signer.signing.sign(&raw);
    let sig_b64 = B64.encode(sig.to_bytes());
    let pubkey = signer.pubkey_b32();
    let url = format!("{}/notify", relay_url.trim_end_matches('/'));
    debug!("[relay] POST {url} category={category} ts={ts}");

    let client = reqwest::Client::new();
    let res = client
        .post(&url)
        .header("content-type", "application/json")
        .header("x-lair-pubkey", &pubkey)
        .header("x-lair-sig",    &sig_b64)
        .body(raw)
        .send()
        .await;
    match res {
        Ok(r) => {
            let status = r.status();
            if status.is_success() {
                debug!("[relay] notify ok: {status}");
            } else {
                let body = r.text().await.unwrap_or_default();
                warn!("[relay] notify {status}: {body}");
            }
        }
        Err(e) => warn!("[relay] notify network error: {e}"),
    }
}

mod hex {
    pub fn encode(bytes: [u8; 16]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(32);
        for b in bytes.iter() {
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0x0f) as usize] as char);
        }
        out
    }
}
