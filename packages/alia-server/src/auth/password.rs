use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};

/// Hash a password using Argon2id (runs in spawn_blocking to avoid blocking async runtime)
pub async fn hash_password(password: &str) -> anyhow::Result<String> {
    let password = password.to_string();
    let hash = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        let hash = argon2
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| anyhow::anyhow!("password hash error: {e}"))?;
        Ok(hash.to_string())
    })
    .await??;
    Ok(hash)
}

/// Verify a password against a hash (runs in spawn_blocking)
pub async fn verify_password(password: &str, hash: &str) -> anyhow::Result<bool> {
    let password = password.to_string();
    let hash = hash.to_string();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<bool> {
        let parsed =
            PasswordHash::new(&hash).map_err(|e| anyhow::anyhow!("invalid password hash: {e}"))?;
        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok())
    })
    .await??;
    Ok(result)
}
