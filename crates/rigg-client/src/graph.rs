//! Microsoft Graph client — the application, service principal and app-role
//! calls that Easy Auth wiring needs (spec
//! `2026-09-09-identity-and-auth-design.md` §5).
//!
//! Graph is route-versioned, so [`GRAPH_BASE_URL`] already carries `/v1.0`
//! and no `api-version` query is ever added. Tokens come from
//! [`crate::auth::token_for`] on the Graph audience, which the Azure CLI
//! serves via `--resource-type ms-graph`.

use std::time::Duration;

use reqwest::{Client, Method};
use serde_json::{Value, json};
use tracing::debug;

use rigg_core::registry::{self, GRAPH_BASE_URL, Provider};

use crate::arm::deterministic_uuid;
use crate::auth::token_for;
use crate::error::ClientError;

/// The app role rigg defines on the application it creates, rather than
/// relying on the all-zeros default role.
const CALLER_ROLE_VALUE: &str = "Caller";

/// An Entra ID application registration.
#[derive(Debug, Clone)]
pub struct Application {
    /// Directory object id — what `PATCH /applications/{id}` takes.
    pub id: String,
    /// Client id (`appId`) — what `api://<appId>` and Easy Auth's
    /// `registration.clientId` take.
    pub app_id: String,
}

/// An enterprise application (service principal).
#[derive(Debug, Clone)]
pub struct ServicePrincipal {
    /// Directory object id — the `resourceId` of an app-role assignment.
    pub id: String,
    /// When true, callers must hold an app role before a token is issued.
    pub app_role_assignment_required: bool,
}

/// The Graph base URL to use: `RIGG_GRAPH_ENDPOINT` when set (trimmed of a
/// trailing `/`), else [`GRAPH_BASE_URL`]. A pure function so it is testable
/// without mutating process env vars.
pub(crate) fn base_url_from(env: Option<&str>) -> String {
    match env {
        Some(v) if !v.is_empty() => v.trim_end_matches('/').to_string(),
        _ => GRAPH_BASE_URL.to_string(),
    }
}

/// Microsoft Graph client.
pub struct GraphClient {
    http: Client,
    token: String,
    base_url: String,
}

impl GraphClient {
    /// Build a client for `tenant` (`None` = the CLI's current tenant).
    pub fn for_tenant(tenant: Option<&str>) -> Result<Self, ClientError> {
        let token = token_for(tenant, registry::provider(Provider::Graph).audience)?;
        Ok(Self::with_token_and_base(
            token,
            base_url_from(std::env::var("RIGG_GRAPH_ENDPOINT").ok().as_deref()),
        ))
    }

    /// Build a client from an already-obtained token and an explicit base
    /// URL — for tests, so Graph-fake tests never touch the process env var.
    pub fn with_token_and_base(token: String, base_url: String) -> Self {
        Self {
            http: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("reqwest client builds"),
            token,
            base_url,
        }
    }

    /// Register a new application. `signInAudience` is single-tenant and
    /// tokens are v2, matching what Easy Auth's issuer expects.
    pub async fn create_application(&self, display_name: &str) -> Result<Application, ClientError> {
        let body = json!({
            "displayName": display_name,
            "signInAudience": "AzureADMyOrg",
            "api": {"requestedAccessTokenVersion": 2}
        });
        let value = self
            .send(Method::POST, "/applications", Some(&body))
            .await?;
        let app_id = string_at(&value, "appId").ok_or_else(|| {
            ClientError::InvalidResponse(
                "Graph created the application but returned no appId".to_string(),
            )
        })?;
        let id = string_at(&value, "id").ok_or_else(|| {
            ClientError::InvalidResponse(
                "Graph created the application but returned no object id".to_string(),
            )
        })?;
        Ok(Application { id, app_id })
    }

    /// Set the application's identifier URI (the `authResourceId` a Web API
    /// skill will ask tokens for) and define the `Caller` app role that
    /// gated enterprise applications require. Returns the app role's id.
    ///
    /// The application is read first and its `appRoles` merged: PATCHing
    /// `appRoles` replaces the collection, so sending only rigg's role would
    /// delete every role the application already publishes (and revoke the
    /// assignments that reference them). A `Caller` role that is already
    /// there is reused — id included, since the assignments in the directory
    /// name it — rather than re-created under a new id.
    pub async fn set_identifier_uri_and_role(
        &self,
        app_object_id: &str,
        uri: &str,
    ) -> Result<String, ClientError> {
        let application = self
            .send(Method::GET, &format!("/applications/{app_object_id}"), None)
            .await?;
        let mut roles = application
            .get("appRoles")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let existing = roles
            .iter()
            .find(|r| {
                r.get("value")
                    .and_then(Value::as_str)
                    .is_some_and(|v| v.eq_ignore_ascii_case(CALLER_ROLE_VALUE))
            })
            .and_then(|r| string_at(r, "id"));
        let role_id = match existing {
            Some(id) => id,
            None => {
                // Deterministic in the URI, so a re-run that does not find
                // the role still lands on the same id.
                let role_id = app_role_id_for(uri);
                roles.push(json!({
                    "id": role_id,
                    "allowedMemberTypes": ["Application"],
                    "displayName": CALLER_ROLE_VALUE,
                    "description": "May call this API",
                    "value": CALLER_ROLE_VALUE,
                    "isEnabled": true
                }));
                role_id
            }
        };
        let body = json!({
            "identifierUris": [uri],
            "api": {"requestedAccessTokenVersion": 2},
            "appRoles": roles
        });
        self.send(
            Method::PATCH,
            &format!("/applications/{app_object_id}"),
            Some(&body),
        )
        .await?;
        Ok(role_id)
    }

    /// The service principal for `app_id`, creating it when the tenant has
    /// none yet.
    pub async fn ensure_service_principal(
        &self,
        app_id: &str,
    ) -> Result<ServicePrincipal, ClientError> {
        let existing = self
            .send(
                Method::GET,
                &format!("/servicePrincipals?$filter=appId%20eq%20'{app_id}'"),
                None,
            )
            .await?;
        if let Some(sp) = existing
            .get("value")
            .and_then(Value::as_array)
            .and_then(|v| v.first())
        {
            return Ok(service_principal_from(sp));
        }
        let created = self
            .send(
                Method::POST,
                "/servicePrincipals",
                Some(&json!({"appId": app_id})),
            )
            .await?;
        Ok(service_principal_from(&created))
    }

    /// Grant `principal_object_id` (a managed identity's object id) the app
    /// role `app_role_id` on the enterprise application `resource_sp_id`.
    pub async fn assign_app_role(
        &self,
        resource_sp_id: &str,
        principal_object_id: &str,
        app_role_id: &str,
    ) -> Result<(), ClientError> {
        let body = json!({
            "principalId": principal_object_id,
            "resourceId": resource_sp_id,
            "appRoleId": app_role_id
        });
        self.send(
            Method::POST,
            &format!("/servicePrincipals/{resource_sp_id}/appRoleAssignedTo"),
            Some(&body),
        )
        .await
        .map(|_| ())
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, ClientError> {
        let url = format!("{}{path}", self.base_url);
        debug!("Graph {method} {url}");
        let mut request = self
            .http
            .request(method, &url)
            .header("Authorization", format!("Bearer {}", self.token));
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await?;
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            return Err(graph_error(status.as_u16(), &text));
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_str(&text)?)
    }
}

/// The app role id rigg uses for one identifier URI — deterministic, so the
/// role survives re-running the wiring.
pub(crate) fn app_role_id_for(uri: &str) -> String {
    deterministic_uuid(&format!("rigg-app-role|{uri}"))
}

fn service_principal_from(value: &Value) -> ServicePrincipal {
    ServicePrincipal {
        id: string_at(value, "id").unwrap_or_default(),
        app_role_assignment_required: value
            .get("appRoleAssignmentRequired")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

fn string_at(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Turn a Graph failure into a `ClientError`, surfacing Graph's own
/// `error.message` — the part that says *which* directory role is missing.
pub(crate) fn graph_error(status: u16, body: &str) -> ClientError {
    let message = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| string_at(v.get("error").unwrap_or(&Value::Null), "message"))
        .unwrap_or_else(|| body.trim().to_string());
    ClientError::Api {
        status,
        message: format!("Microsoft Graph: {message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_base_url_can_be_overridden_for_tests() {
        assert_eq!(
            base_url_from(Some("http://127.0.0.1:1")),
            "http://127.0.0.1:1"
        );
        assert_eq!(
            base_url_from(Some("http://127.0.0.1:1/")),
            "http://127.0.0.1:1"
        );
        assert_eq!(base_url_from(None), GRAPH_BASE_URL);
        assert_eq!(base_url_from(Some("")), GRAPH_BASE_URL);
        // Graph is route-versioned: the base already carries the version and
        // no api-version query is ever appended.
        assert!(GRAPH_BASE_URL.ends_with(registry::GRAPH_API_VERSION));
    }

    #[test]
    fn app_role_id_is_deterministic_in_the_uri_and_uuid_shaped() {
        let a = app_role_id_for("api://app-1");
        assert_eq!(a, app_role_id_for("api://app-1"));
        assert_ne!(a, app_role_id_for("api://app-2"));
        assert_eq!(a.len(), 36);
        assert_eq!(a.chars().filter(|c| *c == '-').count(), 4);
    }

    #[test]
    fn graph_error_surfaces_the_graph_message() {
        let err = graph_error(
            403,
            r#"{"error":{"code":"Authorization_RequestDenied","message":"Insufficient privileges."}}"#,
        );
        assert!(
            err.to_string().contains("Insufficient privileges."),
            "{err}"
        );
        assert!(err.to_string().contains("403"), "{err}");
    }

    #[test]
    fn graph_error_falls_back_to_the_raw_body() {
        let err = graph_error(500, "  upstream exploded  ");
        assert!(err.to_string().contains("upstream exploded"), "{err}");
    }

    #[test]
    fn service_principal_defaults_to_unrestricted() {
        let sp = service_principal_from(&json!({"id": "sp-1"}));
        assert_eq!(sp.id, "sp-1");
        assert!(!sp.app_role_assignment_required);
        let gated =
            service_principal_from(&json!({"id": "sp-2", "appRoleAssignmentRequired": true}));
        assert!(gated.app_role_assignment_required);
    }
}
