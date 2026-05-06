use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: Uuid,
    pub name: String,
    pub role: String,
    pub iat: i64,
    pub exp: i64,
}

pub struct JwtKeys {
    encoding: EncodingKey,
    decoding: DecodingKey,
}

impl JwtKeys {
    pub fn new(secret: &str) -> Self {
        Self {
            encoding: EncodingKey::from_secret(secret.as_bytes()),
            decoding: DecodingKey::from_secret(secret.as_bytes()),
        }
    }

    pub fn encode(&self, user_id: Uuid, name: &str, role: &str, expiry_hours: u64) -> anyhow::Result<String> {
        let now = Utc::now();
        let claims = Claims {
            sub: user_id,
            name: name.to_string(),
            role: role.to_string(),
            iat: now.timestamp(),
            exp: (now + Duration::hours(expiry_hours as i64)).timestamp(),
        };
        let token = encode(&Header::default(), &claims, &self.encoding)?;
        Ok(token)
    }

    pub fn decode(&self, token: &str) -> anyhow::Result<Claims> {
        let data = decode::<Claims>(token, &self.decoding, &Validation::default())?;
        Ok(data.claims)
    }
}
