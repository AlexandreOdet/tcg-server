use serde::Deserialize;

/// Configuration du serveur, lue depuis les variables d'environnement
/// (`DATABASE_URL`, `BIND_ADDR`, `JWT_SECRET`, `APPLE_BUNDLE_ID`).
#[derive(Clone, Deserialize)]
pub struct Config {
    pub database_url: String,
    #[serde(default = "default_bind_addr")]
    pub bind_addr: String,
    /// Secret de signature des access tokens (32 caractères minimum, vérifié au démarrage).
    pub jwt_secret: String,
    /// iOS app bundle ID: audience of Apple tokens. Unset: Apple login disabled.
    pub apple_bundle_id: Option<String>,
    /// Google iOS client ID: audience of Google tokens. Unset: Google login disabled.
    pub google_client_id: Option<String>,
}

fn default_bind_addr() -> String {
    "0.0.0.0:8080".to_owned()
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let config: Self = envy::from_env()?;
        config.validate()?;
        Ok(config)
    }

    /// A server with no login provider would be unusable: refuse to start.
    fn validate(&self) -> anyhow::Result<()> {
        let set = |v: &Option<String>| v.as_deref().is_some_and(|s| !s.trim().is_empty());
        anyhow::ensure!(
            set(&self.apple_bundle_id) || set(&self.google_client_id),
            "configure at least one provider: APPLE_BUNDLE_ID and/or GOOGLE_CLIENT_ID"
        );
        Ok(())
    }
}

// Pas de `Debug` dérivé : l'URL de la base et le secret JWT ne doivent pas finir dans les logs.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("database_url", &"<masquée>")
            .field("bind_addr", &self.bind_addr)
            .field("jwt_secret", &"<masqué>")
            .field("apple_bundle_id", &self.apple_bundle_id)
            .field("google_client_id", &self.google_client_id)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUIRED: [(&str, &str); 2] = [
        ("database_url", "postgres://user:motdepasse@host/db"),
        ("jwt_secret", "un-secret-de-test-d-au-moins-32-caracteres"),
    ];

    fn from_pairs(pairs: &[(&str, &str)]) -> Result<Config, envy::Error> {
        envy::from_iter(pairs.iter().map(|(k, v)| (k.to_uppercase(), v.to_string())))
    }

    fn with(extra: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
        REQUIRED.iter().chain(extra).copied().collect()
    }

    #[test]
    fn bind_addr_has_a_default() {
        let config = from_pairs(&REQUIRED).unwrap();
        assert_eq!(config.bind_addr, "0.0.0.0:8080");
    }

    #[test]
    fn bind_addr_can_be_overridden() {
        let config = from_pairs(&with(&[("bind_addr", "127.0.0.1:3000")])).unwrap();
        assert_eq!(config.bind_addr, "127.0.0.1:3000");
    }

    #[test]
    fn every_required_variable_is_enforced() {
        for missing in REQUIRED.map(|(name, _)| name) {
            let pairs: Vec<_> = REQUIRED
                .iter()
                .filter(|(n, _)| *n != missing)
                .copied()
                .collect();
            assert!(
                from_pairs(&pairs).is_err(),
                "{missing} devrait être obligatoire"
            );
        }
    }

    #[test]
    fn debug_output_hides_secrets() {
        let output = format!(
            "{:?}",
            from_pairs(&with(&[("apple_bundle_id", "fr.exemple.tcg")])).unwrap()
        );
        assert!(!output.contains("motdepasse"));
        assert!(!output.contains("un-secret-de-test"));
        assert!(output.contains("fr.exemple.tcg"));
    }

    #[test]
    fn at_least_one_provider_is_required() {
        assert!(from_pairs(&REQUIRED).unwrap().validate().is_err());
        let blank = from_pairs(&with(&[("google_client_id", "  ")])).unwrap();
        assert!(blank.validate().is_err());
    }

    #[test]
    fn either_provider_alone_is_enough() {
        for extra in [
            ("apple_bundle_id", "fr.exemple.tcg"),
            ("google_client_id", "x.apps"),
        ] {
            assert!(from_pairs(&with(&[extra])).unwrap().validate().is_ok());
        }
    }
}
