use std::io::Read;

use sha2::{Digest, Sha256};
use tokio::sync::OnceCell;

static BUILD_DIGEST: OnceCell<String> = OnceCell::const_new();

/// Hash the actual host artifact, including linked Rust tools. Library hosts
/// are covered as well as the server binary. No filesystem work runs on an
/// async worker, and multiple service instances reuse one measured identity.
pub(super) async fn build_digest() -> anyhow::Result<String> {
    let digest = BUILD_DIGEST
        .get_or_try_init(|| async {
            tokio::task::spawn_blocking(|| -> anyhow::Result<String> {
                let mut file = std::fs::File::open(std::env::current_exe()?)?;
                let mut hash = Sha256::new();
                let mut buffer = [0u8; 64 * 1024];
                loop {
                    let count = file.read(&mut buffer)?;
                    if count == 0 {
                        break;
                    }
                    hash.update(&buffer[..count]);
                }
                Ok(format!("{:x}", hash.finalize()))
            })
            .await?
        })
        .await?;
    Ok(digest.clone())
}
