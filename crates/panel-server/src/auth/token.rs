use rand::RngCore;
use sha2::{Digest, Sha256};

const TOKEN_BYTES: usize = 32;

/// 生成 256-bit 高熵随机 token，hex 编码（64 字符）。
/// 仅在创建节点 / 轮换时一次性返回明文，DB 存 hash_token() 结果。
pub fn generate_token() -> String {
    let mut bytes = [0u8; TOKEN_BYTES];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// SHA-256 hex 摘要。Token 是高熵随机串，无需 Argon2 这类慢哈希；
/// 心跳鉴权每秒可能多次，必须 O(μs) 级别。
pub fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    hex::encode(digest)
}
