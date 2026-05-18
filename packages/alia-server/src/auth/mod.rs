pub mod identity;
pub mod jwt;
pub mod middleware;
pub mod password;
pub mod user_identity;

pub use identity::{ServerKeypair, ensure_server_keypair, load_server_keypair};
pub use jwt::JwtKeys;
pub use middleware::AuthUser;
pub use password::{hash_password, verify_password};
pub use user_identity::{
    EMAIL_IDENTITY_PROVIDER, IdentityAssertion, ResolvedIdentity, VOICEPRINT_IDENTITY_PROVIDER,
    resolve_identity_assertion,
};
