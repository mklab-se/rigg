//! Generic ARM CRUD for Foundry control-plane resource kinds
//! (model deployments, project connections, RAI policies / guardrails)
//! under `Microsoft.CognitiveServices/accounts`, api-version 2026-05-01.

use std::time::Duration;

use reqwest::{Client, Method, StatusCode};
use serde_json::Value;
use tracing::debug;

use rigg_core::registry::{self, Domain};
use rigg_core::resources::ResourceKind;

use crate::arm::ArmClient;
use crate::error::ClientError;

const ARM_BASE_URL: &str = "https://management.azure.com";

/// Where a Foundry account lives in ARM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArmScope {
    pub subscription_id: String,
    pub resource_group: String,
    pub account: String,
}

/// Build the ARM resource URL for a Foundry control-plane kind.
///
/// - Deployment → `accounts/{account}/deployments/{name}`
/// - Guardrail  → `accounts/{account}/raiPolicies/{name}`
/// - Connection → `accounts/{account}/projects/{project}/connections/{name}`
///
/// `name: None` yields the collection URL.
pub fn arm_url(
    scope: &ArmScope,
    kind: ResourceKind,
    project: Option<&str>,
    name: Option<&str>,
) -> Result<String, ClientError> {
    let meta = registry::meta(kind);
    if meta.domain != Domain::FoundryArm {
        return Err(ClientError::Api {
            status: 400,
            message: format!("{kind:?} is not an ARM-managed kind"),
        });
    }
    let mut path = format!(
        "{}/subscriptions/{}/resourceGroups/{}/providers/Microsoft.CognitiveServices/accounts/{}",
        ARM_BASE_URL, scope.subscription_id, scope.resource_group, scope.account
    );
    if kind == ResourceKind::Connection {
        let project = project.ok_or_else(|| ClientError::Api {
            status: 400,
            message: "connections require a Foundry project".to_string(),
        })?;
        path.push_str(&format!("/projects/{}", urlencoding::encode(project)));
    }
    path.push_str(&format!("/{}", meta.collection_path));
    if let Some(name) = name {
        path.push_str(&format!("/{}", urlencoding::encode(name)));
    }
    path.push_str(&format!(
        "?api-version={}",
        registry::ARM_COGNITIVE_API_VERSION
    ));
    Ok(path)
}

/// Generic ARM client for Foundry control-plane kinds.
pub struct ArmResourceClient {
    http: Client,
    token: String,
    scope: ArmScope,
    project: String,
}

impl ArmResourceClient {
    /// Resolve the ARM scope for a Foundry account by name and construct the client.
    pub async fn for_account(account: &str, project: &str) -> Result<Self, ClientError> {
        let arm = ArmClient::new()?;
        let scope = resolve_account_scope(&arm, account).await?;
        Ok(Self {
            http: Client::builder().timeout(Duration::from_secs(30)).build()?,
            token: arm.token().to_string(),
            scope,
            project: project.to_string(),
        })
    }

    /// Test constructor with explicit scope/token/base handled by arm_url.
    pub fn with_token(
        scope: ArmScope,
        project: String,
        token: String,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            http: Client::builder().timeout(Duration::from_secs(30)).build()?,
            token,
            scope,
            project: project.to_string(),
        })
    }

    pub fn scope(&self) -> &ArmScope {
        &self.scope
    }

    fn project_for(&self, kind: ResourceKind) -> Option<&str> {
        (kind == ResourceKind::Connection).then_some(self.project.as_str())
    }

    async fn request(
        &self,
        method: Method,
        url: &str,
        body: Option<&Value>,
    ) -> Result<(StatusCode, Option<Value>), ClientError> {
        let mut req = self
            .http
            .request(method.clone(), url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Content-Type", "application/json");
        if let Some(json) = body {
            req = req.json(json);
        }
        debug!("ARM request: {} {}", method, url);
        let response = req.send().await?;
        let status = response.status();
        let text = response.text().await?;
        if status.is_success() {
            let value = if text.is_empty() {
                None
            } else {
                Some(serde_json::from_str(&text)?)
            };
            Ok((status, value))
        } else if status == StatusCode::NOT_FOUND {
            Err(ClientError::NotFound {
                kind: "arm-resource".to_string(),
                name: url.to_string(),
            })
        } else {
            Err(ClientError::from_response_with_url(
                status.as_u16(),
                &text,
                Some(url),
            ))
        }
    }

    pub async fn list(&self, kind: ResourceKind) -> Result<Vec<Value>, ClientError> {
        let url = arm_url(&self.scope, kind, self.project_for(kind), None)?;
        let (_, body) = self.request(Method::GET, &url, None).await?;
        Ok(body
            .and_then(|v| v.get("value").and_then(|a| a.as_array()).cloned())
            .unwrap_or_default())
    }

    pub async fn get(&self, kind: ResourceKind, name: &str) -> Result<Option<Value>, ClientError> {
        let url = arm_url(&self.scope, kind, self.project_for(kind), Some(name))?;
        match self.request(Method::GET, &url, None).await {
            Ok((_, body)) => Ok(body),
            Err(ClientError::NotFound { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// PUT and poll until the resource reaches a terminal provisioning state.
    pub async fn put(
        &self,
        kind: ResourceKind,
        name: &str,
        body: &Value,
    ) -> Result<Value, ClientError> {
        let url = arm_url(&self.scope, kind, self.project_for(kind), Some(name))?;
        let (status, response) = self.request(Method::PUT, &url, Some(body)).await?;

        // 200/201 with terminal state → done. 201/202 in-progress → poll GET.
        if is_terminal(response.as_ref()) && status != StatusCode::ACCEPTED {
            return response.ok_or_else(|| ClientError::Api {
                status: 500,
                message: "empty ARM PUT response".to_string(),
            });
        }
        self.poll_until_terminal(kind, name).await
    }

    pub async fn delete(&self, kind: ResourceKind, name: &str) -> Result<(), ClientError> {
        let url = arm_url(&self.scope, kind, self.project_for(kind), Some(name))?;
        match self.request(Method::DELETE, &url, None).await {
            Ok(_) => Ok(()),
            Err(ClientError::NotFound { .. }) => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn poll_until_terminal(
        &self,
        kind: ResourceKind,
        name: &str,
    ) -> Result<Value, ClientError> {
        const POLL_INTERVAL: Duration = Duration::from_secs(3);
        const MAX_POLLS: u32 = 100; // 5 minutes
        for _ in 0..MAX_POLLS {
            tokio::time::sleep(POLL_INTERVAL).await;
            if let Some(current) = self.get(kind, name).await? {
                if is_terminal(Some(&current)) {
                    let state = provisioning_state(&current).unwrap_or_default();
                    if state.eq_ignore_ascii_case("succeeded") || state.is_empty() {
                        return Ok(current);
                    }
                    return Err(ClientError::Api {
                        status: 500,
                        message: format!("{kind} '{name}' ended in state '{state}'"),
                    });
                }
            }
        }
        Err(ClientError::Api {
            status: 504,
            message: format!("{kind} '{name}' did not reach a terminal state in time"),
        })
    }
}

fn provisioning_state(v: &Value) -> Option<String> {
    v.get("properties")
        .and_then(|p| p.get("provisioningState"))
        .and_then(|s| s.as_str())
        .map(str::to_string)
}

fn is_terminal(v: Option<&Value>) -> bool {
    match v.and_then(provisioning_state) {
        None => true, // no provisioningState → treat as terminal
        Some(state) => !matches!(
            state.to_ascii_lowercase().as_str(),
            "creating" | "updating" | "deleting" | "accepted" | "running" | "moving"
        ),
    }
}

/// Find the subscription + resource group for a CognitiveServices account name.
pub async fn resolve_account_scope(
    arm: &ArmClient,
    account: &str,
) -> Result<ArmScope, ClientError> {
    for sub in arm.list_subscriptions().await? {
        let accounts = arm.list_ai_services_accounts(&sub.subscription_id).await?;
        for acct in accounts {
            if acct.name.eq_ignore_ascii_case(account) {
                // id: /subscriptions/{s}/resourceGroups/{rg}/providers/...
                let rg = acct
                    .id
                    .split('/')
                    .skip_while(|s| !s.eq_ignore_ascii_case("resourceGroups"))
                    .nth(1)
                    .unwrap_or_default()
                    .to_string();
                if rg.is_empty() {
                    continue;
                }
                return Ok(ArmScope {
                    subscription_id: sub.subscription_id.clone(),
                    resource_group: rg,
                    account: acct.name.clone(),
                });
            }
        }
    }
    Err(ClientError::NotFound {
        kind: "Foundry account".to_string(),
        name: account.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> ArmScope {
        ArmScope {
            subscription_id: "sub-1".into(),
            resource_group: "rg-1".into(),
            account: "mklabaifndr".into(),
        }
    }

    #[test]
    fn deployment_url() {
        let url = arm_url(&scope(), ResourceKind::Deployment, None, Some("gpt-5-mini")).unwrap();
        assert_eq!(
            url,
            "https://management.azure.com/subscriptions/sub-1/resourceGroups/rg-1/providers/Microsoft.CognitiveServices/accounts/mklabaifndr/deployments/gpt-5-mini?api-version=2026-05-01"
        );
    }

    #[test]
    fn guardrail_url() {
        let url = arm_url(&scope(), ResourceKind::Guardrail, None, None).unwrap();
        assert!(url.ends_with("accounts/mklabaifndr/raiPolicies?api-version=2026-05-01"));
    }

    #[test]
    fn connection_url_requires_project() {
        let err = arm_url(&scope(), ResourceKind::Connection, None, Some("c")).unwrap_err();
        assert!(err.to_string().contains("project"));
        let url = arm_url(
            &scope(),
            ResourceKind::Connection,
            Some("proj-default"),
            Some("c"),
        )
        .unwrap();
        assert!(url.contains("/projects/proj-default/connections/c?"));
    }

    #[test]
    fn non_arm_kind_rejected() {
        let err = arm_url(&scope(), ResourceKind::Index, None, None).unwrap_err();
        assert!(err.to_string().contains("not an ARM-managed kind"));
    }

    #[test]
    fn terminal_state_detection() {
        use serde_json::json;
        assert!(is_terminal(Some(
            &json!({"properties": {"provisioningState": "Succeeded"}})
        )));
        assert!(is_terminal(Some(
            &json!({"properties": {"provisioningState": "Failed"}})
        )));
        assert!(!is_terminal(Some(
            &json!({"properties": {"provisioningState": "Creating"}})
        )));
        assert!(is_terminal(Some(&json!({"name": "no-state"}))));
        assert!(is_terminal(None));
    }
}
