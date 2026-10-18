//! Vérification des jetons d'identité OIDC (Sign in with Apple, puis d'autres fournisseurs).

use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::Context;
use async_trait::async_trait;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Deserializer};
use tokio::{sync::RwLock, time::Instant};

use super::hash::sha256_hex;

/// Identité prouvée par un fournisseur.
#[derive(Debug, Clone, PartialEq)]
pub struct VerifiedIdentity {
    pub provider: String,
    /// Identifiant stable chez le fournisseur (claim `sub`).
    pub subject: String,
    pub email: Option<String>,
    pub email_verified: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    /// Le jeton est refusé : mauvaise signature, audience, expiration, nonce…
    #[error("jeton d'identité invalide : {0}")]
    Invalid(String),
    /// Impossible d'obtenir les clés du fournisseur : ce n'est pas la faute du client.
    #[error("clés du fournisseur indisponibles")]
    KeysUnavailable(#[source] anyhow::Error),
}

/// Vérifie un jeton d'identité. Derrière un trait pour injecter un faux vérificateur
/// dans les tests d'intégration et en développement local.
#[async_trait]
pub trait TokenVerifier: Send + Sync {
    /// `raw_nonce` est la valeur brute générée par l'app avant de la transmettre au fournisseur.
    async fn verify(
        &self,
        id_token: &str,
        raw_nonce: &str,
    ) -> Result<VerifiedIdentity, VerifyError>;
}

#[derive(Debug, Clone)]
pub struct OidcConfig {
    /// Nom stocké dans `identities.provider` (ex. `apple`).
    pub provider: String,
    pub issuer: String,
    pub jwks_url: String,
    /// Pour Apple : l'identifiant de bundle de l'app.
    pub audience: String,
    /// `true` : le claim `nonce` contient le SHA-256 hexadécimal du nonce brut (Apple).
    /// `false` : il contient le nonce brut.
    pub nonce_hashed: bool,
}

impl OidcConfig {
    pub fn apple(bundle_id: &str) -> Self {
        Self {
            provider: "apple".into(),
            issuer: "https://appleid.apple.com".into(),
            jwks_url: "https://appleid.apple.com/auth/keys".into(),
            audience: bundle_id.into(),
            nonce_hashed: true,
        }
    }
}

#[derive(Default)]
struct KeyCache {
    by_kid: HashMap<String, Arc<DecodingKey>>,
    fetched_at: Option<Instant>,
}

pub struct OidcVerifier {
    config: OidcConfig,
    http: reqwest::Client,
    cache: RwLock<KeyCache>,
    /// Délai minimal entre deux rafraîchissements : un `kid` inventé par un attaquant
    /// ne doit pas déclencher une requête vers le fournisseur à chaque appel.
    min_refetch_interval: Duration,
}

impl OidcVerifier {
    pub fn new(config: OidcConfig) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .context("client HTTP")?;
        Ok(Self {
            config,
            http,
            cache: RwLock::new(KeyCache::default()),
            min_refetch_interval: Duration::from_secs(60),
        })
    }

    #[cfg(test)]
    fn with_min_refetch_interval(mut self, interval: Duration) -> Self {
        self.min_refetch_interval = interval;
        self
    }

    async fn key_for(&self, kid: &str) -> Result<Arc<DecodingKey>, VerifyError> {
        if let Some(key) = self.cache.read().await.by_kid.get(kid) {
            return Ok(key.clone());
        }

        let mut cache = self.cache.write().await;
        // Un autre appel a pu rafraîchir les clés pendant qu'on attendait le verrou.
        if let Some(key) = cache.by_kid.get(kid) {
            return Ok(key.clone());
        }
        if cache
            .fetched_at
            .is_some_and(|at| at.elapsed() < self.min_refetch_interval)
        {
            return Err(VerifyError::Invalid("clé de signature inconnue".into()));
        }

        cache.by_kid = self
            .fetch_keys()
            .await
            .map_err(VerifyError::KeysUnavailable)?;
        cache.fetched_at = Some(Instant::now());
        cache
            .by_kid
            .get(kid)
            .cloned()
            .ok_or_else(|| VerifyError::Invalid("clé de signature inconnue".into()))
    }

    async fn fetch_keys(&self) -> anyhow::Result<HashMap<String, Arc<DecodingKey>>> {
        let jwks: Jwks = self
            .http
            .get(&self.config.jwks_url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .context("JWKS illisible")?;

        jwks.keys
            .into_iter()
            .filter(|jwk| jwk.kty == "RSA")
            .map(|jwk| {
                let key = DecodingKey::from_rsa_components(&jwk.n, &jwk.e)
                    .with_context(|| format!("clé {} invalide", jwk.kid))?;
                Ok((jwk.kid, Arc::new(key)))
            })
            .collect()
    }
}

#[async_trait]
impl TokenVerifier for OidcVerifier {
    async fn verify(
        &self,
        id_token: &str,
        raw_nonce: &str,
    ) -> Result<VerifiedIdentity, VerifyError> {
        let invalid = |reason: &str| VerifyError::Invalid(reason.to_owned());

        let header = decode_header(id_token).map_err(|_| invalid("en-tête illisible"))?;
        // On n'accepte que RS256 : jamais l'algorithme annoncé par le jeton lui-même.
        if header.alg != Algorithm::RS256 {
            return Err(invalid("algorithme non pris en charge"));
        }
        let kid = header
            .kid
            .ok_or_else(|| invalid("identifiant de clé absent"))?;
        let key = self.key_for(&kid).await?;

        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[&self.config.issuer]);
        validation.set_audience(&[&self.config.audience]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);

        let claims = decode::<Claims>(id_token, &key, &validation)
            .map_err(|error| VerifyError::Invalid(error.to_string()))?
            .claims;

        let expected_nonce = if self.config.nonce_hashed {
            sha256_hex(raw_nonce)
        } else {
            raw_nonce.to_owned()
        };
        if claims.nonce.as_deref() != Some(expected_nonce.as_str()) {
            return Err(invalid("nonce incorrect"));
        }

        Ok(VerifiedIdentity {
            provider: self.config.provider.clone(),
            subject: claims.sub,
            email: claims.email,
            email_verified: claims.email_verified.unwrap_or(false),
        })
    }
}

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

#[derive(Deserialize)]
struct Jwk {
    kty: String,
    kid: String,
    n: String,
    e: String,
}

#[derive(Deserialize)]
struct Claims {
    sub: String,
    nonce: Option<String>,
    email: Option<String>,
    #[serde(default, deserialize_with = "bool_or_string")]
    email_verified: Option<bool>,
}

/// Apple envoie parfois `email_verified` sous forme de chaîne (`"true"`), Google en booléen.
fn bool_or_string<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<bool>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum BoolOrString {
        Bool(bool),
        Text(String),
    }
    Ok(
        Option::<BoolOrString>::deserialize(deserializer)?.map(|value| match value {
            BoolOrString::Bool(b) => b,
            BoolOrString::Text(s) => s.eq_ignore_ascii_case("true"),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::test_keys::*;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use serde_json::{json, Value};
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
    };

    const BUNDLE_ID: &str = "fr.exemple.tcg";
    const NONCE: &str = "nonce-brut-genere-par-l-app";

    fn now() -> u64 {
        jsonwebtoken::get_current_timestamp()
    }

    fn jwks(keys: &[(&str, &str)]) -> Value {
        let keys: Vec<Value> = keys
            .iter()
            .map(|(kid, modulus)| {
                json!({ "kty": "RSA", "kid": kid, "use": "sig", "alg": "RS256",
                        "n": modulus, "e": EXPONENT })
            })
            .collect();
        json!({ "keys": keys })
    }

    /// Jeton valide par défaut, que chaque test altère sur un point précis.
    fn claims() -> Value {
        json!({
            "iss": "https://appleid.apple.com",
            "aud": BUNDLE_ID,
            "sub": "001234.abcdef",
            "exp": now() + 3600,
            "iat": now(),
            "nonce": sha256_hex(NONCE),
            "email": "joueur@privaterelay.appleid.com",
            "email_verified": "true",
        })
    }

    fn sign(claims: &Value, kid: &str, private_pem: &str) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(kid.into());
        encode(
            &header,
            claims,
            &EncodingKey::from_rsa_pem(private_pem.as_bytes()).unwrap(),
        )
        .unwrap()
    }

    async fn server_with(keys: Value) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/keys"))
            .respond_with(ResponseTemplate::new(200).set_body_json(keys))
            .mount(&server)
            .await;
        server
    }

    fn verifier(server: &MockServer) -> OidcVerifier {
        let mut config = OidcConfig::apple(BUNDLE_ID);
        config.jwks_url = format!("{}/keys", server.uri());
        OidcVerifier::new(config).unwrap()
    }

    async fn requests_to_jwks(server: &MockServer) -> usize {
        server.received_requests().await.unwrap().len()
    }

    #[tokio::test]
    async fn accepts_a_valid_token() {
        let server = server_with(jwks(&[("kid-a", key_a_modulus())])).await;
        let token = sign(&claims(), "kid-a", key_a_private_pem());

        let identity = verifier(&server).verify(&token, NONCE).await.unwrap();
        assert_eq!(
            identity,
            VerifiedIdentity {
                provider: "apple".into(),
                subject: "001234.abcdef".into(),
                email: Some("joueur@privaterelay.appleid.com".into()),
                email_verified: true, // reçu comme la chaîne "true"
            }
        );
    }

    #[tokio::test]
    async fn email_is_optional() {
        let server = server_with(jwks(&[("kid-a", key_a_modulus())])).await;
        let mut claims = claims();
        claims.as_object_mut().unwrap().remove("email");
        claims.as_object_mut().unwrap().remove("email_verified");
        let token = sign(&claims, "kid-a", key_a_private_pem());

        let identity = verifier(&server).verify(&token, NONCE).await.unwrap();
        assert_eq!(identity.email, None);
        assert!(!identity.email_verified);
    }

    #[tokio::test]
    async fn email_verified_as_a_real_boolean_is_accepted() {
        let server = server_with(jwks(&[("kid-a", key_a_modulus())])).await;
        let mut claims = claims();
        claims["email_verified"] = json!(true);
        let token = sign(&claims, "kid-a", key_a_private_pem());

        assert!(
            verifier(&server)
                .verify(&token, NONCE)
                .await
                .unwrap()
                .email_verified
        );
    }

    /// Chaque altération doit être refusée par `Invalid` (et non une erreur serveur).
    async fn assert_rejected(claims: Value, signing_pem: &str, raw_nonce: &str) {
        let server = server_with(jwks(&[("kid-a", key_a_modulus())])).await;
        let token = sign(&claims, "kid-a", signing_pem);
        let result = verifier(&server).verify(&token, raw_nonce).await;
        assert!(matches!(result, Err(VerifyError::Invalid(_))), "{result:?}");
    }

    #[tokio::test]
    async fn rejects_a_wrong_audience() {
        let mut claims = claims();
        claims["aud"] = json!("fr.autre.app");
        assert_rejected(claims, key_a_private_pem(), NONCE).await;
    }

    #[tokio::test]
    async fn rejects_a_wrong_issuer() {
        let mut claims = claims();
        claims["iss"] = json!("https://accounts.evil.example");
        assert_rejected(claims, key_a_private_pem(), NONCE).await;
    }

    #[tokio::test]
    async fn rejects_an_expired_token() {
        let mut claims = claims();
        claims["exp"] = json!(now() - 3600);
        assert_rejected(claims, key_a_private_pem(), NONCE).await;
    }

    #[tokio::test]
    async fn rejects_a_token_signed_with_another_key() {
        // Même `kid` mais signé avec la clé B : la signature ne correspond pas à la clé publiée.
        assert_rejected(claims(), key_b_private_pem(), NONCE).await;
    }

    #[tokio::test]
    async fn rejects_a_wrong_nonce() {
        assert_rejected(claims(), key_a_private_pem(), "un-autre-nonce").await;
    }

    #[tokio::test]
    async fn rejects_a_token_without_nonce() {
        let mut claims = claims();
        claims.as_object_mut().unwrap().remove("nonce");
        assert_rejected(claims, key_a_private_pem(), NONCE).await;
    }

    #[tokio::test]
    async fn rejects_a_token_without_subject() {
        let mut claims = claims();
        claims.as_object_mut().unwrap().remove("sub");
        assert_rejected(claims, key_a_private_pem(), NONCE).await;
    }

    #[tokio::test]
    async fn rejects_garbage_and_symmetric_algorithms() {
        let server = server_with(jwks(&[("kid-a", key_a_modulus())])).await;
        let verifier = verifier(&server);

        let result = verifier.verify("pas.un.jwt", NONCE).await;
        assert!(matches!(result, Err(VerifyError::Invalid(_))));

        // Attaque classique : un jeton HS256 signé avec la clé publique comme secret.
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some("kid-a".into());
        let forged = encode(
            &header,
            &claims(),
            &EncodingKey::from_secret(key_a_modulus().as_bytes()),
        )
        .unwrap();
        let result = verifier.verify(&forged, NONCE).await;
        assert!(matches!(result, Err(VerifyError::Invalid(_))));
    }

    #[tokio::test]
    async fn fetches_the_keys_once_and_caches_them() {
        let server = server_with(jwks(&[("kid-a", key_a_modulus())])).await;
        let verifier = verifier(&server);
        let token = sign(&claims(), "kid-a", key_a_private_pem());

        for _ in 0..3 {
            verifier.verify(&token, NONCE).await.unwrap();
        }
        assert_eq!(requests_to_jwks(&server).await, 1);
    }

    #[tokio::test]
    async fn refreshes_the_keys_when_the_provider_rotates_them() {
        let server = MockServer::start().await;
        // 1re réponse : seulement la clé A ; ensuite : A et B.
        Mock::given(method("GET"))
            .and(path("/keys"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(jwks(&[("kid-a", key_a_modulus())])),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/keys"))
            .respond_with(ResponseTemplate::new(200).set_body_json(jwks(&[
                ("kid-a", key_a_modulus()),
                ("kid-b", key_b_modulus()),
            ])))
            .mount(&server)
            .await;
        let verifier = verifier(&server).with_min_refetch_interval(Duration::ZERO);

        let token_a = sign(&claims(), "kid-a", key_a_private_pem());
        verifier.verify(&token_a, NONCE).await.unwrap();

        let token_b = sign(&claims(), "kid-b", key_b_private_pem());
        verifier.verify(&token_b, NONCE).await.unwrap();
        assert_eq!(requests_to_jwks(&server).await, 2);
    }

    #[tokio::test]
    async fn an_unknown_kid_cannot_trigger_a_refetch_on_every_call() {
        let server = server_with(jwks(&[("kid-a", key_a_modulus())])).await;
        let verifier = verifier(&server); // intervalle de 60 s
        let unknown = sign(&claims(), "kid-inconnu", key_a_private_pem());

        for _ in 0..5 {
            let result = verifier.verify(&unknown, NONCE).await;
            assert!(matches!(result, Err(VerifyError::Invalid(_))));
        }
        assert_eq!(requests_to_jwks(&server).await, 1);
    }

    #[tokio::test]
    async fn provider_outage_is_not_reported_as_an_invalid_token() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let token = sign(&claims(), "kid-a", key_a_private_pem());

        let result = verifier(&server).verify(&token, NONCE).await;
        assert!(
            matches!(result, Err(VerifyError::KeysUnavailable(_))),
            "{result:?}"
        );
    }
}
