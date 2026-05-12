use std::path::Path;

use anyhow::{Context, Result, bail};
use ed25519_dalek::SigningKey;

#[derive(Clone)]
pub struct ServerKeypair {
    signing_key: SigningKey,
}

impl ServerKeypair {
    pub fn public_key_bytes(&self) -> [u8; 32] {
        self.signing_key.verifying_key().to_bytes()
    }

    pub fn signing_key(&self) -> &SigningKey {
        &self.signing_key
    }
}

pub fn ensure_server_keypair(path: &Path) -> Result<ServerKeypair> {
    if path.exists() {
        return load_server_keypair(path);
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating key directory: {}", parent.display()))?;
    }

    let secret_key: [u8; 32] = rand::random();
    let signing_key = SigningKey::from_bytes(&secret_key);
    let public_key = signing_key.verifying_key().to_bytes();

    let mut encoded = Vec::with_capacity(64);
    encoded.extend_from_slice(&secret_key);
    encoded.extend_from_slice(&public_key);

    std::fs::write(path, &encoded)
        .with_context(|| format!("writing server keypair: {}", path.display()))?;
    set_owner_only_permissions(path)?;

    Ok(ServerKeypair { signing_key })
}

pub fn load_server_keypair(path: &Path) -> Result<ServerKeypair> {
    let encoded = std::fs::read(path)
        .with_context(|| format!("reading server keypair: {}", path.display()))?;

    if encoded.len() != 32 && encoded.len() != 64 {
        bail!(
            "server keypair at {} has invalid length {}, expected 32 or 64 bytes",
            path.display(),
            encoded.len()
        );
    }

    let secret_key: [u8; 32] = encoded[..32]
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid server secret key length"))?;
    let signing_key = SigningKey::from_bytes(&secret_key);

    if encoded.len() == 64 {
        let expected = signing_key.verifying_key().to_bytes();
        if encoded[32..] != expected {
            bail!(
                "server keypair at {} has mismatched public key bytes",
                path.display()
            );
        }
    }

    set_owner_only_permissions(path)?;

    Ok(ServerKeypair { signing_key })
}

#[cfg(unix)]
fn set_owner_only_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let permissions = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, permissions)
        .with_context(|| format!("setting permissions on {}", path.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_owner_only_permissions(_path: &Path) -> Result<()> {
    Ok(())
}
