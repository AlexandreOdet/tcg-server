//! Clés RSA **de test**, générées à la volée (une fois par exécution des tests) pour signer de
//! faux jetons d'identité. Aucune clé n'est écrite dans le dépôt.

use std::sync::OnceLock;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand_core_06::OsRng;
use rsa::{
    pkcs8::{EncodePrivateKey, LineEnding},
    traits::PublicKeyParts,
    RsaPrivateKey,
};

/// Exposant public RSA (65537) en base64url.
pub const EXPONENT: &str = "AQAB";

struct TestKey {
    private_pem: String,
    modulus: String,
}

fn generate() -> TestKey {
    let key = RsaPrivateKey::new(&mut OsRng, 2048).expect("génération de la clé de test");
    TestKey {
        private_pem: key
            .to_pkcs8_pem(LineEnding::LF)
            .expect("export PEM")
            .to_string(),
        modulus: URL_SAFE_NO_PAD.encode(key.n().to_bytes_be()),
    }
}

fn key_a() -> &'static TestKey {
    static KEY: OnceLock<TestKey> = OnceLock::new();
    KEY.get_or_init(generate)
}

fn key_b() -> &'static TestKey {
    static KEY: OnceLock<TestKey> = OnceLock::new();
    KEY.get_or_init(generate)
}

pub fn key_a_private_pem() -> &'static str {
    &key_a().private_pem
}
pub fn key_a_modulus() -> &'static str {
    &key_a().modulus
}
pub fn key_b_private_pem() -> &'static str {
    &key_b().private_pem
}
pub fn key_b_modulus() -> &'static str {
    &key_b().modulus
}
