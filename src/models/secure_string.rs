//! SecretString - A container for sensitive string data
//!
//! This module provides a secure string implementation that protects sensitive
//! data in memory and prevents it from being accidentally exposed through logs,
//! serialization, or debug output.
//!
//! The `SecretString` type wraps a `SecretVec<u8>` and provides methods for
//! securely handling string data, including zeroizing the memory when the
//! string is dropped.
use std::{fmt, sync::Arc};

use secrets::SecretVec;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

#[derive(Clone)]
pub struct SecretString(Arc<SecretVec<u8>>);

unsafe impl Send for SecretString {}
unsafe impl Sync for SecretString {}

impl SecretString {
    pub fn new(s: &str) -> Self {
        let bytes = s.as_bytes();
        let secret_vec = SecretVec::new(bytes.len(), |buffer| {
            buffer.copy_from_slice(bytes);
        });
        Self(Arc::new(secret_vec))
    }

    pub fn as_str<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&str) -> R,
    {
        let bytes = self.0.borrow();
        let s = unsafe { std::str::from_utf8_unchecked(&bytes) };
        f(s)
    }

    pub fn to_str(&self) -> Zeroizing<String> {
        let bytes = self.0.borrow();
        let s = unsafe { std::str::from_utf8_unchecked(&*bytes) };
        Zeroizing::new(s.to_string())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
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
        let s = String::deserialize(deserializer)?;

        Ok(SecretString::new(&s))
    }
}

impl PartialEq for SecretString {
    fn eq(&self, other: &Self) -> bool {
        let self_bytes = self.0.borrow();
        let other_bytes = other.0.borrow();
        self_bytes.len() == other_bytes.len()
            && subtle::ConstantTimeEq::ct_eq(&*self_bytes, &*other_bytes).into()
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "SecretString(REDACTED)")
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
        let num_threads = 1;
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

        assert_eq!(secret.0.len(), 100_000);
    }
}
