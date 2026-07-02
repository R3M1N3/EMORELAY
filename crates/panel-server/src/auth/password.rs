use anyhow::{anyhow, Result};
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use std::sync::OnceLock;

pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow!("argon2 hash: {e}"))?;
    Ok(hash.to_string())
}

pub fn verify_password(password: &str, hash: &str) -> Result<bool> {
    let parsed = PasswordHash::new(hash).map_err(|e| anyhow!("invalid password hash: {e}"))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

/// 用户名不存在时跑一次 verify 对齐时延，防止用户名枚举侧信道。
/// 第一次调用时一次性生成一个 Argon2 hash，之后复用；返回的 hash 不会匹配任何真实输入。
pub fn dummy_hash() -> &'static str {
    static H: OnceLock<String> = OnceLock::new();
    H.get_or_init(|| {
        hash_password("dummy-password-for-timing-oracle-defense-only")
            .expect("generate dummy hash")
    })
    .as_str()
}

/// [`verify_password`] 的异步封装:Argon2 刻意 CPU 密集,直接在 async handler 里跑会堵住
/// Tokio worker 线程,并发登录时可拖垮运行时。放到 spawn_blocking 线程池执行。
/// 入参取所有权(spawn_blocking 闭包需 'static)。
pub async fn verify_password_blocking(password: String, hash: String) -> Result<bool> {
    tokio::task::spawn_blocking(move || verify_password(&password, &hash))
        .await
        .map_err(|e| anyhow!("verify_password blocking join: {e}"))?
}

/// [`hash_password`] 的异步封装,理由同 [`verify_password_blocking`]。
pub async fn hash_password_blocking(password: String) -> Result<String> {
    tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(|e| anyhow!("hash_password blocking join: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    // 阻塞封装:hash 产出的散列能被 verify 接受,错误密码被拒(与同步版语义一致)。
    #[tokio::test]
    async fn blocking_hash_then_verify_roundtrip() {
        let hash = hash_password_blocking("correct-horse-8".to_string())
            .await
            .unwrap();
        assert!(
            verify_password_blocking("correct-horse-8".to_string(), hash.clone())
                .await
                .unwrap()
        );
        assert!(
            !verify_password_blocking("wrong-password".to_string(), hash)
                .await
                .unwrap()
        );
    }

    // 阻塞 verify 与同步 hash 互通:同一散列,阻塞校验通过。
    #[tokio::test]
    async fn blocking_verify_agrees_with_sync_hash() {
        let hash = hash_password("abcd1234").unwrap();
        assert!(
            verify_password_blocking("abcd1234".to_string(), hash)
                .await
                .unwrap()
        );
    }
}
