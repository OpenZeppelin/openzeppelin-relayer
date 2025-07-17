use lazy_static::lazy_static;
use regex::Regex;

pub const MINIMUM_SECRET_VALUE_LENGTH: usize = 32;

// Regex for validating notification IDs
lazy_static! {
    pub static ref ID_REGEX: Regex = Regex::new(r"^[a-zA-Z0-9-_]+$").unwrap();
}
