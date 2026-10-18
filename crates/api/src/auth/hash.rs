//! Petits utilitaires de hachage partagés par l'authentification.

use sha2::{Digest, Sha256};

pub fn sha256(input: &[u8]) -> [u8; 32] {
    Sha256::digest(input).into()
}

/// SHA-256 en hexadécimal minuscule (format du nonce que l'app envoie à Apple).
pub fn sha256_hex(input: &str) -> String {
    sha256(input.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_hex_matches_a_known_vector() {
        assert_eq!(
            sha256_hex("hello world"),
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }
}
