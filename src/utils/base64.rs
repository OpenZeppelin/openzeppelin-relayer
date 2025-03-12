// base64 encode and decode helper functions

use base64::Engine;

pub fn base64_encode(message: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(message)
}
pub fn base64_decode(data: &str) -> Result<Vec<u8>, base64::DecodeError> {
    base64::engine::general_purpose::STANDARD.decode(data)
}
