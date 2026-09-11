use std::collections::HashMap;

use jsonwebtoken::{
    Algorithm, DecodingKey, Validation, decode, decode_header,
    jwk::{JwkSet, KeyAlgorithm, PublicKeyUse},
};
use serde::Deserialize;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ActorRole {
    Human,
    Agent,
}

impl ActorRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Agent => "agent",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Actor {
    pub id: Uuid,
    pub role: ActorRole,
    pub expires_at: i64,
}

#[derive(Clone)]
pub struct AuthValidator {
    issuer: String,
    audience: String,
    keys: HashMap<String, DecodingKey>,
}

#[derive(Debug)]
pub struct AuthConfigurationError;

impl std::fmt::Display for AuthConfigurationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("invalid OIDC authentication configuration")
    }
}

impl std::error::Error for AuthConfigurationError {}

#[derive(Deserialize)]
struct Claims {
    sub: String,
    n2n_role: ActorRole,
    exp: i64,
}

impl AuthValidator {
    pub fn new(
        issuer: impl Into<String>,
        audience: impl Into<String>,
        jwks_json: &str,
    ) -> Result<Self, AuthConfigurationError> {
        let issuer = issuer.into();
        let audience = audience.into();
        if issuer.is_empty() || audience.is_empty() {
            return Err(AuthConfigurationError);
        }

        let jwks: JwkSet = serde_json::from_str(jwks_json).map_err(|_| AuthConfigurationError)?;
        let mut keys = HashMap::new();
        for jwk in jwks.keys {
            if jwk.common.public_key_use != Some(PublicKeyUse::Signature)
                || jwk.common.key_algorithm != Some(KeyAlgorithm::RS256)
            {
                continue;
            }
            let Some(key_id) = jwk
                .common
                .key_id
                .clone()
                .filter(|key_id| !key_id.is_empty())
            else {
                continue;
            };
            let key = DecodingKey::from_jwk(&jwk).map_err(|_| AuthConfigurationError)?;
            if keys.insert(key_id, key).is_some() {
                return Err(AuthConfigurationError);
            }
        }
        if keys.is_empty() {
            return Err(AuthConfigurationError);
        }

        Ok(Self {
            issuer,
            audience,
            keys,
        })
    }

    pub fn authenticate_bearer(&self, authorization: Option<&str>) -> Option<Actor> {
        let (scheme, token) = authorization?.split_once(' ')?;
        if !scheme.eq_ignore_ascii_case("Bearer") {
            return None;
        }
        if token.is_empty() || token.contains(char::is_whitespace) {
            return None;
        }
        let header = decode_header(token).ok()?;
        if header.alg != Algorithm::RS256 {
            return None;
        }
        let key = self.keys.get(header.kid.as_deref()?)?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&[&self.audience]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation.leeway = 0;
        validation.reject_tokens_expiring_in_less_than = 1;
        validation.validate_nbf = true;
        let claims = decode::<Claims>(token, key, &validation).ok()?.claims;

        Some(Actor {
            id: Uuid::parse_str(&claims.sub).ok()?,
            role: claims.n2n_role,
            expires_at: claims.exp,
        })
    }
}
