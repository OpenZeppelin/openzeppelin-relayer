use rand::RngCore;

pub fn unsafe_generate_random_private_key() -> Vec<u8> {
    let mut rng = rand::rng();
    let mut pk = vec![0u8; 32];
    rng.fill_bytes(pk.as_mut_slice());
    pk
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_private_key_length() {
        let pk = unsafe_generate_random_private_key();
        assert_eq!(pk.len(), 32, "Private key should be 32 bytes");
    }

    #[test]
    fn test_private_key_uniqueness() {
        let mut keys = HashSet::new();
        for _ in 0..100 {
            let pk = unsafe_generate_random_private_key();
            assert!(keys.insert(pk), "Generated private key should be unique");
        }
    }

    #[test]
    fn test_private_key_not_zero() {
        let pk = unsafe_generate_random_private_key();
        assert!(
            !pk.iter().all(|&byte| byte == 0),
            "Private key should not be all zeros"
        );
    }
}
