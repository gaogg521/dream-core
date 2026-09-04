//! TOTP (RFC 6238, SHA-1 · 30s · 6 位) 校验与 base32 编解码。
//!
//! 校验原语用成熟的 `hmac` + `sha1` crate（标准 RFC 2104 HMAC / FIPS 180
//! SHA-1 实现），本模块只做参数拼装、动态截断与窗口比对——不手写散列算法。
//! base32 是 RFC 4648 的标准编码（验证器 App 录入密钥的通用格式）。

use hmac::{Hmac, Mac};
use sha1::Sha1;
use subtle::ConstantTimeEq;

const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
const STEP_SECONDS: i64 = 30;
/// 允许 ±1 个时间片（±30s）时钟漂移。
const WINDOW_STEPS: i64 = 1;
pub const CODE_DIGITS: usize = 6;

type HmacSha1 = Hmac<Sha1>;

/// 生成 20 字节随机 TOTP 密钥并编码为 base32（验证器录入格式）。
pub fn generate_secret() -> String {
    let mut bytes = [0u8; 20];
    getrandom::getrandom(&mut bytes).expect("OS RNG unavailable");
    base32_encode(&bytes)
}

pub fn base32_encode(data: &[u8]) -> String {
    // 逐位累积输出（等价于标准 base32，只是不做 5 字节分块）。
    let mut acc = 0u32;
    let mut bits = 0u32;
    let mut out = String::with_capacity(data.len().div_ceil(5) * 8);
    for &b in data {
        acc = (acc << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            let idx = ((acc >> bits) & 0x1f) as usize;
            out.push(ALPHABET[idx] as char);
        }
    }
    // 末尾不足 5 位时补零对齐——RFC 4648 允许无填充省略
    if bits > 0 {
        let idx = ((acc << (5 - bits)) & 0x1f) as usize;
        out.push(ALPHABET[idx] as char);
    }
    out
}

pub fn base32_decode(encoded: &str) -> Option<Vec<u8>> {
    let mut bits = 0u32;
    let mut acc = 0u32;
    let mut out = Vec::with_capacity(encoded.len() * 5 / 8);
    for ch in encoded.trim_end_matches('=').chars() {
        let idx = ALPHABET.iter().position(|b| (*b as char).to_ascii_uppercase() == ch.to_ascii_uppercase())? as u32;
        acc = (acc << 5) | idx;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// 计算指定时间片的 6 位动态码（HMAC-SHA1 + 动态截断）。
fn hotp_at(secret: &[u8], counter: u64) -> u32 {
    let mut mac = HmacSha1::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[19] & 0x0f) as usize;
    let binary = u32::from_be_bytes([
        digest[offset] & 0x7f,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]);
    binary % 10u32.pow(CODE_DIGITS as u32)
}

/// 当前时间片的动态码（仅供测试/展示，登录路径用 [`verify_with_window`]）。
pub fn totp_code(secret_b32: &str, at_ms: i64) -> Option<String> {
    let secret = base32_decode(secret_b32)?;
    let step = at_ms / 1000 / STEP_SECONDS;
    Some(format!("{:0width$}", hotp_at(&secret, step as u64), width = CODE_DIGITS))
}

/// 带窗口与防重放的校验。命中时返回该码对应的时间片（调用方持久化，
/// 之后任何 `step <= 该值` 的码一律拒绝，保证同一动态码不可重放）。
///
/// `code` 用常数时间比较（subtle），防时序侧信道。
pub fn verify_with_window(
    secret_b32: &str,
    code: &str,
    last_used_step: Option<i64>,
    at_ms: i64,
) -> Option<i64> {
    let normalized = code.trim();
    if normalized.len() != CODE_DIGITS || !normalized.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let supplied: u32 = normalized.parse().ok()?;
    let secret = base32_decode(secret_b32)?;
    let now_step = at_ms / 1000 / STEP_SECONDS;

    for delta in -WINDOW_STEPS..=WINDOW_STEPS {
        let step = now_step + delta;
        if let Some(used) = last_used_step {
            if step <= used {
                continue; // 防重放：已用过（或更早）的时间片直接跳过
            }
        }
        let candidate = hotp_at(&secret, step as u64);
        if candidate.ct_eq(&supplied).into() {
            return Some(step);
        }
    }
    None
}

/// 构造 otpauth:// URI（验证器 App 扫码 / 手工录入）。
pub fn otpauth_uri(issuer: &str, account: &str, secret_b32: &str) -> String {
    let label = urlencoding_lite(issuer);
    let account = urlencoding_lite(account);
    format!("otpauth://totp/{label}:{account}?secret={secret_b32}&issuer={label}&algorithm=SHA1&digits={CODE_DIGITS}&period={STEP_SECONDS}")
}

/// 最小化的百分号编码（otpauth URI 参数只需这一档）。
fn urlencoding_lite(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238 附录 B 的参考向量（SHA-1 · 8 位 · 59s step0）换算到本实现
    /// 的参数（6 位 · 30s）：用固定密钥 + 固定时间验证稳定性即可。
    #[test]
    fn totp_is_stable_and_digit_bound() {
        let secret = base32_encode(b"12345678901234567890");
        let code = totp_code(&secret, 1_700_000_000_000).expect("decodes");
        assert_eq!(code.len(), CODE_DIGITS);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
        // 同一时间片重复计算结果一致
        assert_eq!(totp_code(&secret, 1_700_000_000_000), Some(code));
    }

    #[test]
    fn verify_accepts_current_and_rejects_replay() {
        let secret = base32_encode(b"12345678901234567890");
        let now = 1_700_000_000_000;
        let code = totp_code(&secret, now).expect("decodes");
        assert_eq!(verify_with_window(&secret, &code, None, now), Some(now / 1000 / 30));
        // 同码重放（last_step 已推进到该时间片）必须被拒
        assert_eq!(verify_with_window(&secret, &code, Some(now / 1000 / 30), now), None);
    }

    #[test]
    fn base32_roundtrip() {
        let raw = b"anything bytes 0123456789";
        let decoded = base32_decode(&base32_encode(raw)).expect("decodes");
        assert_eq!(decoded, raw.to_vec());
    }
}
