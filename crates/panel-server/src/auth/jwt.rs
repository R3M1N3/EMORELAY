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
    /// token 作用域:""=普通登录(全权);"sub"=仅订阅用量端点(I4)。
    #[serde(default)]
    pub scope: String,
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
        scope: String::new(),
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;
    Ok(token)
}

/// 签发订阅专用 token:scope="sub",仅可访问 /api/subscription/usage。
/// exp_unix 由调用方给(到账号有效期;无到期则远期)。
pub fn encode_sub_token(secret: &str, user_id: i64, username: &str, exp_unix: i64) -> Result<String> {
    let claims = Claims {
        sub: user_id,
        username: username.to_string(),
        role: "user".to_string(),
        exp: exp_unix,
        mcp: false,
        scope: "sub".to_string(),
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
