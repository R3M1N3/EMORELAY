use anyhow::Result;
use chrono::Utc;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: i64,
    pub username: String,
    pub role: String,
    pub exp: i64,
    /// must-change-password:为 true 时只允许访问 me/change-password,其余路由拒绝(I1)。
    #[serde(default)]
    pub mcp: bool,
}

pub fn encode_jwt(
    secret: &str,
    user_id: i64,
    username: &str,
    role: &str,
    expiry_hours: u64,
    mcp: bool,
) -> Result<String> {
    let exp = (Utc::now() + chrono::Duration::hours(expiry_hours as i64)).timestamp();
    let claims = Claims {
        sub: user_id,
        username: username.to_string(),
        role: role.to_string(),
        exp,
        mcp,
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;
    Ok(token)
}

pub fn decode_jwt(secret: &str, token: &str) -> Result<Claims> {
    let mut validation = Validation::default();
    validation.set_required_spec_claims(&["exp"]);
    let data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )?;
    Ok(data.claims)
}
