// base64 encode and decode helper functions

use base64::Engine;

pub fn base64_encode(message: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(message)
}
pub fn base64_decode(data: &str) -> Result<Vec<u8>, base64::DecodeError> {
    base64::engine::general_purpose::STANDARD.decode(data)
}

pub fn base64_url_encode(message: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(message)
}
pub fn base64_url_decode(data: &str) -> Result<Vec<u8>, base64::DecodeError> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base64_encode() {
        assert_eq!(base64_encode(b"Hello, world!"), "SGVsbG8sIHdvcmxkIQ==");
    }

    #[test]
    fn test_base64_decode() {
        let decoded = base64_decode("SGVsbG8sIHdvcmxkIQ==").unwrap();
        assert_eq!(decoded, b"Hello, world!");
    }
}
