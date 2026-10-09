use crate::error::AuthError;

const MIN_PASSWORD_LENGTH: usize = 8;
const MAX_PASSWORD_LENGTH: usize = 128;
const MIN_USERNAME_LENGTH: usize = 3;
const MAX_USERNAME_LENGTH: usize = 32;

/// Common weak passwords rejected during validation (compared lowercased).
///
/// Every entry is at least `MIN_PASSWORD_LENGTH` characters — shorter ones are
/// already refused by the length rule. Drawn from the most frequent entries of
/// public breach corpora plus the admin defaults people reach for on a fresh
/// enterprise install; the original five-entry list let `Password123`,
/// `admin888`, `1qaz2wsx` through.
const WEAK_PASSWORDS: &[&str] = &[
    "password",
    "password1",
    "password12",
    "password123",
    "password1234",
    "password!",
    "password@123",
    "passw0rd",
    "p@ssw0rd",
    "p@ssword",
    "p@ssword1",
    "p@ssw0rd123",
    "pa$$w0rd",
    "passwd123",
    "12345678",
    "123456789",
    "1234567890",
    "0123456789",
    "87654321",
    "987654321",
    "11111111",
    "00000000",
    "88888888",
    "66666666",
    "12341234",
    "123123123",
    "12344321",
    "11223344",
    "1q2w3e4r",
    "1q2w3e4r5t",
    "1qaz2wsx",
    "1qaz2wsx3edc",
    "qazwsxedc",
    "zaq12wsx",
    "q1w2e3r4",
    "qwertyui",
    "qwertyuiop",
    "qwerty123",
    "qwerty12",
    "qwer1234",
    "asdfghjk",
    "asdf1234",
    "zxcvbnm1",
    "abcdefgh",
    "abcd1234",
    "abc12345",
    "abc123456",
    "a1b2c3d4",
    "aa123456",
    "a12345678",
    "admin123",
    "admin888",
    "admin1234",
    "admin12345",
    "admin@123",
    "administrator",
    "root1234",
    "changeme",
    "changeme1",
    "welcome1",
    "welcome123",
    "iloveyou",
    "iloveyou1",
    "sunshine",
    "princess",
    "football",
    "baseball",
    "superman",
    "trustno1",
    "letmein1",
    "monkey123",
    "dragon123",
    "master123",
    "starwars",
    "whatever",
    "computer",
    "internet",
    "test1234",
    "test12345",
    "testtest",
    "guest123",
    "default1",
    "secret123",
    "woaini520",
    "woaini1314",
    "5201314520",
    "13145200",
    "onework123",
    "dream1234",
];

const MIN_DISTINCT_CHARS: usize = 3;

/// Validate password strength.
///
/// Rules:
/// - Length: 8-128 characters (Unicode scalar values — `len()` counts bytes,
///   which let a three-character CJK password pass the 8 minimum)
/// - Not in the weak password blacklist (case-insensitive)
/// - Not a single character repeated, or nearly so (`aaaaaaab`)
pub fn validate_password(password: &str) -> Result<(), AuthError> {
    let length = password.chars().count();
    if length < MIN_PASSWORD_LENGTH {
        return Err(AuthError::WeakPassword(format!(
            "Password must be at least {MIN_PASSWORD_LENGTH} characters"
        )));
    }
    if length > MAX_PASSWORD_LENGTH {
        return Err(AuthError::WeakPassword(format!(
            "Password must not exceed {MAX_PASSWORD_LENGTH} characters"
        )));
    }
    let lower = password.to_lowercase();
    if WEAK_PASSWORDS.contains(&lower.as_str()) {
        return Err(AuthError::WeakPassword("Password is too common".into()));
    }
    let distinct = lower.chars().collect::<std::collections::HashSet<_>>().len();
    if distinct < MIN_DISTINCT_CHARS {
        return Err(AuthError::WeakPassword(
            "Password must not be the same character repeated".into(),
        ));
    }
    Ok(())
}

/// Validate username format.
///
/// Rules:
/// - Length: 3-32 characters
/// - Allowed characters: `[a-zA-Z0-9_-]`
/// - Must not start or end with `-` or `_`
pub fn validate_username(username: &str) -> Result<(), AuthError> {
    if username.len() < MIN_USERNAME_LENGTH {
        return Err(AuthError::InvalidUsername(format!(
            "Username must be at least {MIN_USERNAME_LENGTH} characters"
        )));
    }
    if username.len() > MAX_USERNAME_LENGTH {
        return Err(AuthError::InvalidUsername(format!(
            "Username must not exceed {MAX_USERNAME_LENGTH} characters"
        )));
    }
    if !username
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(AuthError::InvalidUsername(
            "Username may only contain letters, digits, underscores, and hyphens".into(),
        ));
    }
    // Safe to index: length >= 3, all ASCII
    let first = username.as_bytes()[0];
    let last = username.as_bytes()[username.len() - 1];
    if matches!(first, b'-' | b'_') || matches!(last, b'-' | b'_') {
        return Err(AuthError::InvalidUsername(
            "Username must not start or end with a hyphen or underscore".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Password validation ---

    #[test]
    fn valid_password() {
        assert!(validate_password("StrongP@ss1").is_ok());
    }

    #[test]
    fn password_exactly_min_length_valid() {
        assert!(validate_password("abcDEF12").is_ok());
    }

    #[test]
    fn password_exactly_max_length() {
        let max = "ab1".repeat(43)[..128].to_string();
        assert!(validate_password(&max).is_ok());
    }

    #[test]
    fn password_too_short() {
        assert!(matches!(validate_password("short"), Err(AuthError::WeakPassword(_))));
    }

    #[test]
    fn password_too_long() {
        let long = "a".repeat(129);
        assert!(matches!(validate_password(&long), Err(AuthError::WeakPassword(_))));
    }

    #[test]
    fn weak_password_rejected() {
        for &weak in WEAK_PASSWORDS {
            assert!(validate_password(weak).is_err(), "expected rejection for: {weak}");
        }
    }

    /// `len()` is bytes: three CJK characters are 9 bytes and used to pass.
    #[test]
    fn length_is_counted_in_characters_not_bytes() {
        assert!(matches!(validate_password("密码好"), Err(AuthError::WeakPassword(_))));
        assert!(validate_password("这是一个够长的密码").is_ok());
        // 128 CJK characters are 384 bytes and must still be accepted.
        let max = "密码长".repeat(43).chars().take(128).collect::<String>();
        assert!(validate_password(&max).is_ok());
    }

    #[test]
    fn weak_list_catches_the_reported_examples() {
        for weak in [
            "Password123",
            "admin888",
            "iloveyou",
            "1qaz2wsx",
            "P@ssw0rd",
            "Admin@123",
        ] {
            assert!(validate_password(weak).is_err(), "expected rejection for: {weak}");
        }
    }

    #[test]
    fn weak_list_entries_are_all_long_enough_to_matter() {
        for &weak in WEAK_PASSWORDS {
            assert!(
                weak.chars().count() >= MIN_PASSWORD_LENGTH,
                "{weak} is shorter than the minimum"
            );
            assert_eq!(weak, weak.to_lowercase(), "{weak} must be lowercase to ever match");
        }
    }

    #[test]
    fn repeated_character_passwords_are_rejected() {
        assert!(validate_password("aaaaaaaa").is_err());
        assert!(validate_password("abababab").is_err());
        assert!(validate_password("abcabcab").is_ok());
    }

    #[test]
    fn weak_password_case_insensitive() {
        assert!(validate_password("PASSWORD").is_err());
        assert!(validate_password("Password").is_err());
    }

    // --- Username validation ---

    #[test]
    fn valid_username() {
        assert!(validate_username("test_user-1").is_ok());
    }

    #[test]
    fn username_alphanumeric_only() {
        assert!(validate_username("abc123").is_ok());
    }

    #[test]
    fn username_exactly_min_length() {
        assert!(validate_username("abc").is_ok());
    }

    #[test]
    fn username_exactly_max_length() {
        let max = "a".repeat(32);
        assert!(validate_username(&max).is_ok());
    }

    #[test]
    fn username_too_short() {
        assert!(matches!(validate_username("ab"), Err(AuthError::InvalidUsername(_))));
    }

    #[test]
    fn username_too_long() {
        let long = "a".repeat(33);
        assert!(matches!(validate_username(&long), Err(AuthError::InvalidUsername(_))));
    }

    #[test]
    fn username_invalid_chars() {
        assert!(validate_username("test@user").is_err());
        assert!(validate_username("test user").is_err());
        assert!(validate_username("test.user").is_err());
    }

    #[test]
    fn username_starts_with_hyphen() {
        assert!(validate_username("-test").is_err());
    }

    #[test]
    fn username_starts_with_underscore() {
        assert!(validate_username("_test").is_err());
    }

    #[test]
    fn username_ends_with_hyphen() {
        assert!(validate_username("test-").is_err());
    }

    #[test]
    fn username_ends_with_underscore() {
        assert!(validate_username("test_").is_err());
    }

    #[test]
    fn username_hyphen_in_middle() {
        assert!(validate_username("test-user").is_ok());
    }

    #[test]
    fn username_underscore_in_middle() {
        assert!(validate_username("test_user").is_ok());
    }
}
