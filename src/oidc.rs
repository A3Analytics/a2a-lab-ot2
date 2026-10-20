//! Optional `OpenID` Connect protection for the A2A listener.

use std::sync::Arc;

use a2a_lab_dev_kit::{
    A2aLabError, A2aServer, HttpAuthSecurityScheme, OidcAuthenticator, OpenIdConnectSecurityScheme,
    SecurityScheme,
};

/// Issuer and audience used when A2A requires a bearer token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcConfig {
    /// Issuer expected on access tokens.
    pub issuer: String,
    /// Audience expected on access tokens.
    pub audience: String,
    /// Scope required on access tokens.
    pub scope: String,
    /// Discovery document URL. Defaults to `{issuer}/.well-known/openid-configuration`.
    pub discovery_url: Option<String>,
}

/// Advertises `OpenID` Connect and HTTP bearer as alternative requirements.
///
/// Both requirements use the same authenticator and the configured scope.
/// The bearer alternative lets a browser send the access token. It does not
/// add a login flow, and it does not change how the token is validated.
pub fn with_oidc(server: A2aServer, config: &OidcConfig) -> Result<A2aServer, A2aLabError> {
    let mut authenticator = OidcAuthenticator::new(&config.issuer, &config.audience)?;
    if let Some(discovery) = config
        .discovery_url
        .as_deref()
        .filter(|discovery| !discovery.is_empty())
    {
        authenticator = authenticator.with_discovery_url(discovery);
    }
    let discovery = authenticator.discovery_url().to_owned();
    let scope = config.scope.clone();
    Ok(server
        .with_security(
            [
                (
                    "oidc".to_owned(),
                    SecurityScheme::OpenIdConnect(OpenIdConnectSecurityScheme {
                        open_id_connect_url: discovery,
                        description: Some("OpenID Connect client credentials".to_owned()),
                    }),
                ),
                (
                    "bearer".to_owned(),
                    SecurityScheme::HttpAuth(HttpAuthSecurityScheme {
                        scheme: "bearer".to_owned(),
                        description: Some(
                            "OpenID Connect access token presented as an HTTP bearer credential"
                                .to_owned(),
                        ),
                        bearer_format: Some("JWT".to_owned()),
                    }),
                ),
            ]
            .into(),
            vec![
                [("oidc".to_owned(), vec![scope.clone()])].into(),
                [("bearer".to_owned(), vec![scope])].into(),
            ],
        )
        .with_authenticator(Arc::new(authenticator)))
}
