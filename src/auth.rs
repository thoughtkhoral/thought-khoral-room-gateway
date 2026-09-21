use std::collections::HashMap;

use jsonwebtoken::{
    Algorithm, DecodingKey, Validation, decode, decode_header,
    jwk::{JwkSet, KeyAlgorithm, PublicKeyUse},
};
use serde::Deserialize;
use uuid::Uuid;

use crate::config::{AGENT_GATEWAY_AUDIENCE, AGENT_GATEWAY_CLIENT_ID};

const MAX_WORKLOAD_TOKEN_LIFETIME_SECONDS: i64 = 300;

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Actor {
    pub id: Uuid,
    pub role: ActorRole,
    pub display_name: String,
    pub expires_at: i64,
}

/// Authentication failures for the internal workload-only surface.  This is deliberately
/// separate from `Actor`: a Keycloak service account is never a room participant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkloadAuthenticationError {
    Unauthenticated,
    Forbidden,
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
    #[serde(rename = "n2n_role")]
    role: ActorRole,
    exp: i64,
    name: Option<String>,
    preferred_username: Option<String>,
}

#[derive(Deserialize)]
struct WorkloadClaims {
    aud: WorkloadAudience,
    azp: Option<String>,
    iat: Option<i64>,
    exp: i64,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum WorkloadAudience {
    One(String),
    Many(Vec<String>),
}

pub(crate) fn display_name_for(
    role: ActorRole,
    actor_id: Uuid,
    name: Option<&str>,
    preferred_username: Option<&str>,
) -> String {
    let role_label = match role {
        ActorRole::Human => "Human",
        ActorRole::Agent => "Agent",
    };
    name.or(preferred_username)
        .and_then(|value| {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.chars().take(128).collect())
        })
        .unwrap_or_else(|| format!("{role_label} {}", &actor_id.to_string()[..8]))
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
        self.bearer_token(authorization)
            .and_then(|token| self.authenticate_access_token(token))
    }

    pub fn authenticate_access_token(&self, token: &str) -> Option<Actor> {
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

        let actor_id = Uuid::parse_str(&claims.sub).ok()?;
        Some(Actor {
            id: actor_id,
            role: claims.role,
            display_name: display_name_for(
                claims.role,
                actor_id,
                claims.name.as_deref(),
                claims.preferred_username.as_deref(),
            ),
            expires_at: claims.exp,
        })
    }

    /// Accepts only a short-lived client-credentials token minted for the agent gateway.
    /// It returns no room identity and cannot be used to enter the WebSocket room flow.
    pub fn authenticate_agent_gateway_bearer(
        &self,
        authorization: Option<&str>,
    ) -> Result<(), WorkloadAuthenticationError> {
        let token = self
            .bearer_token(authorization)
            .ok_or(WorkloadAuthenticationError::Unauthenticated)?;
        let header =
            decode_header(token).map_err(|_| WorkloadAuthenticationError::Unauthenticated)?;
        if header.alg != Algorithm::RS256 {
            return Err(WorkloadAuthenticationError::Unauthenticated);
        }
        let key = self
            .keys
            .get(
                header
                    .kid
                    .as_deref()
                    .ok_or(WorkloadAuthenticationError::Unauthenticated)?,
            )
            .ok_or(WorkloadAuthenticationError::Unauthenticated)?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&[AGENT_GATEWAY_AUDIENCE]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub", "iat"]);
        validation.leeway = 0;
        validation.reject_tokens_expiring_in_less_than = 1;
        validation.validate_nbf = true;
        let claims = decode::<WorkloadClaims>(token, key, &validation)
            .map_err(|_| WorkloadAuthenticationError::Unauthenticated)?
            .claims;

        let audience = match &claims.aud {
            WorkloadAudience::One(audience) => audience,
            WorkloadAudience::Many(audiences) => {
                let _multiple_audiences = audiences;
                return Err(WorkloadAuthenticationError::Unauthenticated);
            }
        };
        if audience != AGENT_GATEWAY_AUDIENCE {
            return Err(WorkloadAuthenticationError::Unauthenticated);
        }
        if claims.azp.as_deref() != Some(AGENT_GATEWAY_CLIENT_ID) {
            return Err(WorkloadAuthenticationError::Forbidden);
        }
        let issued_at = claims.iat.ok_or(WorkloadAuthenticationError::Forbidden)?;
        let now = chrono::Utc::now().timestamp();
        if issued_at > now
            || claims.exp <= issued_at
            || claims.exp.saturating_sub(issued_at) > MAX_WORKLOAD_TOKEN_LIFETIME_SECONDS
        {
            return Err(WorkloadAuthenticationError::Forbidden);
        }
        Ok(())
    }

    fn bearer_token<'a>(&self, authorization: Option<&'a str>) -> Option<&'a str> {
        let (scheme, token) = authorization?.split_once(' ')?;
        if !scheme.eq_ignore_ascii_case("Bearer")
            || token.is_empty()
            || token.contains(char::is_whitespace)
        {
            return None;
        }
        Some(token)
    }
}

#[cfg(test)]
mod tests {
    use super::{ActorRole, display_name_for};
    use uuid::Uuid;

    #[test]
    fn prefers_trimmed_name_then_username_then_role_and_short_id() {
        let actor_id = Uuid::parse_str("12345678-1234-4234-8234-123456789abc").unwrap();
        assert_eq!(
            display_name_for(
                ActorRole::Human,
                actor_id,
                Some("  Maya Chen  "),
                Some("maya"),
            ),
            "Maya Chen"
        );
        assert_eq!(
            display_name_for(ActorRole::Agent, actor_id, None, Some("atlas")),
            "atlas"
        );
        assert_eq!(
            display_name_for(ActorRole::Human, actor_id, None, None),
            "Human 12345678"
        );
    }
}
