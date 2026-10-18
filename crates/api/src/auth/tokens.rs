//! Jetons de session : access JWT court (HS256) et refresh tokens opaques.

use anyhow::ensure;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use jsonwebtoken::{
    decode, encode, get_current_timestamp, Algorithm, DecodingKey, EncodingKey, Header, Validation,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::hash::sha256;

const ISSUER: &str = "tcg-server";
/// Durée de vie d'un access token : courte, car il n'est pas révocable.
pub const ACCESS_TTL_SECS: u64 = 15 * 60;
const MIN_SECRET_LEN: usize = 32;

#[derive(Serialize, Deserialize)]
struct AccessClaims {
    sub: Uuid,
    iss: String,
    iat: u64,
    exp: u64,
}

pub struct TokenService {
    encoding: EncodingKey,
    decoding: DecodingKey,
}

impl TokenService {
    pub fn new(secret: &str) -> anyhow::Result<Self> {
        ensure!(
            secret.len() >= MIN_SECRET_LEN,
            "JWT_SECRET trop court : {MIN_SECRET_LEN} caractères minimum"
        );
        Ok(Self {
            encoding: EncodingKey::from_secret(secret.as_bytes()),
            decoding: DecodingKey::from_secret(secret.as_bytes()),
        })
    }

    pub fn issue_access(&self, user_id: Uuid) -> anyhow::Result<String> {
        self.issue_access_at(user_id, get_current_timestamp())
    }

    pub fn issue_access_at(&self, user_id: Uuid, now: u64) -> anyhow::Result<String> {
        let claims = AccessClaims {
            sub: user_id,
            iss: ISSUER.to_owned(),
            iat: now,
            exp: now + ACCESS_TTL_SECS,
        };
        Ok(encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &self.encoding,
        )?)
    }

    /// Renvoie l'identifiant de l'utilisateur si le jeton est authentique et non expiré.
    pub fn verify_access(&self, token: &str) -> Result<Uuid, jsonwebtoken::errors::Error> {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.set_issuer(&[ISSUER]);
        validation.set_required_spec_claims(&["exp", "iss", "sub"]);
        validation.leeway = 0; // émis et vérifié par le même serveur : pas de décalage d'horloge
        Ok(decode::<AccessClaims>(token, &self.decoding, &validation)?
            .claims
            .sub)
    }
}

/// Refresh token : 32 octets aléatoires (CSPRNG) en base64url. Seul son hash est stocké.
pub fn new_refresh_token() -> String {
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Les refresh tokens sont aléatoires et à haute entropie : un SHA-256 simple suffit
/// (pas besoin d'un hachage lent comme pour des mots de passe).
pub fn hash_refresh_token(token: &str) -> [u8; 32] {
    sha256(token.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "un-secret-de-test-d-au-moins-32-caracteres";

    fn service() -> TokenService {
        TokenService::new(SECRET).unwrap()
    }

    #[test]
    fn an_issued_access_token_verifies_back_to_the_same_user() {
        let user = Uuid::new_v4();
        let token = service().issue_access(user).unwrap();
        assert_eq!(service().verify_access(&token).unwrap(), user);
    }

    #[test]
    fn an_expired_access_token_is_rejected() {
        let issued_long_ago = get_current_timestamp() - ACCESS_TTL_SECS - 10;
        let token = service()
            .issue_access_at(Uuid::new_v4(), issued_long_ago)
            .unwrap();
        assert!(service().verify_access(&token).is_err());
    }

    #[test]
    fn a_token_signed_with_another_secret_is_rejected() {
        let other = TokenService::new("un-tout-autre-secret-de-plus-de-32-caracteres").unwrap();
        let token = other.issue_access(Uuid::new_v4()).unwrap();
        assert!(service().verify_access(&token).is_err());
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(service().verify_access("pas.un.jwt").is_err());
        assert!(service().verify_access("").is_err());
    }

    #[test]
    fn a_short_secret_is_refused() {
        assert!(TokenService::new("trop-court").is_err());
    }

    #[test]
    fn refresh_tokens_are_unique_and_long_enough() {
        let (a, b) = (new_refresh_token(), new_refresh_token());
        assert_ne!(a, b);
        assert_eq!(a.len(), 43); // 32 octets en base64url sans padding
    }

    #[test]
    fn refresh_token_hash_is_deterministic_and_token_specific() {
        assert_eq!(hash_refresh_token("abc"), hash_refresh_token("abc"));
        assert_ne!(hash_refresh_token("abc"), hash_refresh_token("abd"));
    }
}
