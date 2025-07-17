//! SecretString - A container for sensitive string data
//!
//! This module provides a secure string implementation that protects sensitive
//! data in memory and prevents it from being accidentally exposed through logs,
//! serialization, or debug output.
//!
//! The `SecretString` type wraps a `SecretVec<u8>` and provides methods for
//! securely handling string data, including zeroizing the memory when the
//! string is dropped.
use std::{fmt, sync::Mutex};

use secrets::SecretVec;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use zeroize::Zeroizing;

pub struct SecretString(Mutex<SecretVec<u8>>);

impl Clone for SecretString {
    fn clone(&self) -> Self {
        let secret_vec = self.with_secret_vec(|secret_vec| secret_vec.clone());
        Self(Mutex::new(secret_vec))
    }
}

impl SecretString {
    /// Creates a new SecretString from a regular string
    ///
    /// The input string's content is copied into secure memory and protected.
    pub fn new(s: &str) -> Self {
        let bytes = Zeroizing::new(s.as_bytes().to_vec());
        let secret_vec = SecretVec::new(bytes.len(), |buffer| {
            buffer.copy_from_slice(&bytes);
        });
        Self(Mutex::new(secret_vec))
    }

    /// Access the SecretVec with a provided function
    ///
    /// This is a private helper method to safely access the locked SecretVec
    fn with_secret_vec<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&SecretVec<u8>) -> R,
    {
        let guard = match self.0.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        f(&guard)
    }

    /// Access the secret string content with a provided function
    ///
    /// This method allows temporary access to the string content
    /// without creating a copy of the string.
    pub fn as_str<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&str) -> R,
    {
        self.with_secret_vec(|secret_vec| {
            let bytes = secret_vec.borrow();
            let s = unsafe { std::str::from_utf8_unchecked(&bytes) };
            f(s)
        })
    }

    /// Create a temporary copy of the string content
    ///
    /// Returns a zeroizing string that will be securely erased when dropped.
    /// Only use this when absolutely necessary as it creates a copy of the secret.
    pub fn to_str(&self) -> Zeroizing<String> {
        self.with_secret_vec(|secret_vec| {
            let bytes = secret_vec.borrow();
            let s = unsafe { std::str::from_utf8_unchecked(&bytes) };
            Zeroizing::new(s.to_string())
        })
    }

    /// Check if the secret string is empty
    ///
    /// Returns true if the string contains no bytes.
    pub fn is_empty(&self) -> bool {
        self.with_secret_vec(|secret_vec| secret_vec.is_empty())
    }

    /// Check if the secret string meets a minimum length requirement
    ///
    /// Returns true if the string has at least the specified length.
    pub fn has_minimum_length(&self, min_length: usize) -> bool {
        self.with_secret_vec(|secret_vec| {
            let bytes = secret_vec.borrow();
            bytes.len() >= min_length
        })
    }
}

impl Serialize for SecretString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str("REDACTED")
    }
}

impl<'de> Deserialize<'de> for SecretString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = Zeroizing::new(String::deserialize(deserializer)?);

        Ok(SecretString::new(&s))
    }
}

impl PartialEq for SecretString {
    fn eq(&self, other: &Self) -> bool {
        self.with_secret_vec(|self_vec| {
            other.with_secret_vec(|other_vec| {
                let self_bytes = self_vec.borrow();
                let other_bytes = other_vec.borrow();

                self_bytes.len() == other_bytes.len()
                    && subtle::ConstantTimeEq::ct_eq(&*self_bytes, &*other_bytes).into()
            })
        })
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "SecretString(REDACTED)")
    }
}

impl ToSchema for SecretString {
    fn name() -> std::borrow::Cow<'static, str> {
        "SecretString".into()
    }
}

impl utoipa::PartialSchema for SecretString {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::Schema> {
        use utoipa::openapi::*;

        RefOr::T(Schema::Object(
            ObjectBuilder::new()
                .schema_type(schema::Type::String)
                .format(Some(schema::SchemaFormat::KnownFormat(
                    schema::KnownFormat::Password,
                )))
                .description(Some("A secret string value (content is protected)"))
                .build(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::thread;

    #[test]
    fn test_new_creates_valid_secret_string() {
        let secret = SecretString::new("test_secret_value");

        secret.as_str(|s| {
            assert_eq!(s, "test_secret_value");
        });
    }

    #[test]
    fn test_empty_string_is_handled_correctly() {
        let empty = SecretString::new("");

        assert!(empty.is_empty());

        empty.as_str(|s| {
            assert_eq!(s, "");
        });
    }

    #[test]
    fn test_to_str_creates_correct_zeroizing_copy() {
        let secret = SecretString::new("temporary_copy");

        let copy = secret.to_str();

        assert_eq!(&*copy, "temporary_copy");
    }

    #[test]
    fn test_is_empty_returns_correct_value() {
        let empty = SecretString::new("");
        let non_empty = SecretString::new("not empty");

        assert!(empty.is_empty());
        assert!(!non_empty.is_empty());
    }

    #[test]
    fn test_serialization_redacts_content() {
        let secret = SecretString::new("should_not_appear_in_serialized_form");

        let serialized = serde_json::to_string(&secret).unwrap();

        assert_eq!(serialized, "\"REDACTED\"");
        assert!(!serialized.contains("should_not_appear_in_serialized_form"));
    }

    #[test]
    fn test_deserialization_creates_valid_secret_string() {
        let json_str = "\"deserialized_secret\"";

        let deserialized: SecretString = serde_json::from_str(json_str).unwrap();

        deserialized.as_str(|s| {
            assert_eq!(s, "deserialized_secret");
        });
    }

    #[test]
    fn test_equality_comparison_works_correctly() {
        let secret1 = SecretString::new("same_value");
        let secret2 = SecretString::new("same_value");
        let secret3 = SecretString::new("different_value");

        assert_eq!(secret1, secret2);
        assert_ne!(secret1, secret3);
    }

    #[test]
    fn test_debug_output_redacts_content() {
        let secret = SecretString::new("should_not_appear_in_debug");

        let debug_str = format!("{:?}", secret);

        assert_eq!(debug_str, "SecretString(REDACTED)");
        assert!(!debug_str.contains("should_not_appear_in_debug"));
    }

    #[test]
    fn test_thread_safety() {
        let secret = SecretString::new("shared_across_threads");
        let num_threads = 10;
        let barrier = Arc::new(Barrier::new(num_threads));
        let mut handles = vec![];

        for i in 0..num_threads {
            let thread_secret = secret.clone();
            let thread_barrier = barrier.clone();

            let handle = thread::spawn(move || {
                // Wait for all threads to be ready
                thread_barrier.wait();

                // Verify the secret content
                thread_secret.as_str(|s| {
                    assert_eq!(s, "shared_across_threads");
                });

                // Test other methods
                assert!(!thread_secret.is_empty());
                let copy = thread_secret.to_str();
                assert_eq!(&*copy, "shared_across_threads");

                // Return thread ID to verify all threads ran
                i
            });

            handles.push(handle);
        }

        // Verify all threads completed successfully
        let mut completed_threads = vec![];
        for handle in handles {
            completed_threads.push(handle.join().unwrap());
        }

        // Sort results to make comparison easier
        completed_threads.sort();
        assert_eq!(completed_threads, (0..num_threads).collect::<Vec<_>>());
    }

    #[test]
    fn test_unicode_handling() {
        let unicode_string = "こんにちは世界!";
        let secret = SecretString::new(unicode_string);

        secret.as_str(|s| {
            assert_eq!(s, unicode_string);
            assert_eq!(s.chars().count(), 8); // 7 Unicode characters + 1 ASCII
        });
    }

    #[test]
    fn test_special_characters_handling() {
        let special_chars = "!@#$%^&*()_+{}|:<>?~`-=[]\\;',./";
        let secret = SecretString::new(special_chars);

        secret.as_str(|s| {
            assert_eq!(s, special_chars);
        });
    }

    #[test]
    fn test_very_long_string() {
        // Create a long string (100,000 characters)
        let long_string = "a".repeat(100_000);
        let secret = SecretString::new(&long_string);

        secret.as_str(|s| {
            assert_eq!(s.len(), 100_000);
            assert_eq!(s, long_string);
        });

        assert_eq!(secret.0.lock().unwrap().len(), 100_000);
    }

    #[test]
    fn test_has_minimum_length() {
        // Create test strings of various lengths
        let empty = SecretString::new("");
        let short = SecretString::new("abc");
        let medium = SecretString::new("abcdefghij"); // 10 characters
        let long = SecretString::new("abcdefghijklmnopqrst"); // 20 characters

        // Test with minimum length 0
        assert!(empty.has_minimum_length(0));
        assert!(short.has_minimum_length(0));
        assert!(medium.has_minimum_length(0));
        assert!(long.has_minimum_length(0));

        // Test with minimum length 1
        assert!(!empty.has_minimum_length(1));
        assert!(short.has_minimum_length(1));
        assert!(medium.has_minimum_length(1));
        assert!(long.has_minimum_length(1));

        // Test with exact length matches
        assert!(empty.has_minimum_length(0));
        assert!(short.has_minimum_length(3));
        assert!(medium.has_minimum_length(10));
        assert!(long.has_minimum_length(20));

        // Test with length exceeding the string
        assert!(!empty.has_minimum_length(1));
        assert!(!short.has_minimum_length(4));
        assert!(!medium.has_minimum_length(11));
        assert!(!long.has_minimum_length(21));

        // Test with significantly larger minimum length
        assert!(!short.has_minimum_length(100));
        assert!(!medium.has_minimum_length(100));
        assert!(!long.has_minimum_length(100));
    }
}
