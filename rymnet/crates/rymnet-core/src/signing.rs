use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::Serialize;
use serde_json::Value;

pub fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(value).context("serialize canonical JSON")
}

pub fn sign_json_value(signing_key: &SigningKey, value: &Value) -> Result<String> {
    let bytes = canonical_json(value)?;
    let signature = signing_key.sign(&bytes);
    Ok(STANDARD.encode(signature.to_bytes()))
}

pub fn verify_json_value(public_key_b64: &str, signature_b64: &str, value: &Value) -> Result<bool> {
    let public_key = STANDARD
        .decode(public_key_b64)
        .context("decode public key")?;
    let public_key: [u8; 32] = public_key
        .try_into()
        .map_err(|_| anyhow::anyhow!("public key must be 32 bytes"))?;
    let signature = STANDARD.decode(signature_b64).context("decode signature")?;
    let signature: [u8; 64] = signature
        .try_into()
        .map_err(|_| anyhow::anyhow!("signature must be 64 bytes"))?;

    let verifying_key = VerifyingKey::from_bytes(&public_key).context("parse verifying key")?;
    let signature = Signature::from_bytes(&signature);
    let bytes = canonical_json(value)?;
    Ok(verifying_key.verify(&bytes, &signature).is_ok())
}
