//! Client error types

use thiserror::Error;

use crate::auth::AuthError;

/// Azure Search client errors
#[derive(Debug, Error)]
pub enum ClientError {
    #[error("Authentication error: {0}")]
    Auth(#[from] AuthError),

    #[error("HTTP request failed: {0}")]
    Request(#[from] reqwest::Error),

    #[error("API error ({status}): {message}")]
    Api { status: u16, message: String },

    #[error("Access denied (403 Forbidden): {service}")]
    Forbidden {
        service: String,
        message: String,
        body: String,
    },

    #[error("Resource not found: {kind} '{name}'")]
    NotFound { kind: String, name: String },

    #[error("Resource already exists: {kind} '{name}'")]
    AlreadyExists { kind: String, name: String },

    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    #[error("Rate limited, retry after {retry_after} seconds")]
    RateLimited { retry_after: u64 },

    #[error("Service unavailable: {0}")]
    ServiceUnavailable(String),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

impl ClientError {
    /// Create an API error from HTTP status, response body, and request URL
    pub fn from_response(status: u16, body: &str) -> Self {
        Self::from_response_with_url(status, body, None)
    }

    /// Create an API error with the originating URL for richer diagnostics
    pub fn from_response_with_url(status: u16, body: &str, url: Option<&str>) -> Self {
        // Extract message from Azure error format
        let parsed_message = serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|json| {
                json.get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .map(String::from)
            });

        // For 403, create a Forbidden error with actionable context
        if status == 403 {
            let service = url
                .and_then(|u| u.strip_prefix("https://").and_then(|s| s.split('/').next()))
                .unwrap_or("unknown service")
                .to_string();
            let message = parsed_message.unwrap_or_default();
            return Self::Forbidden {
                service,
                message,
                body: body.to_string(),
            };
        }

        if let Some(message) = parsed_message {
            return Self::Api { status, message };
        }

        // Provide a human-readable fallback when the body is empty
        let message = if body.trim().is_empty() {
            format!("HTTP {} with no error details from the server", status)
        } else {
            body.to_string()
        };

        Self::Api { status, message }
    }

    /// Check if this error is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            ClientError::RateLimited { .. } | ClientError::ServiceUnavailable(_)
        )
    }

    /// Get suggested action for this error
    pub fn suggestion(&self) -> &'static str {
        match self {
            ClientError::Auth(AuthError::NotLoggedIn) => {
                "Run 'az login' to authenticate with Azure CLI"
            }
            ClientError::Auth(AuthError::AzCliNotFound) => {
                "Install Azure CLI: https://docs.microsoft.com/cli/azure/install-azure-cli"
            }
            ClientError::Auth(AuthError::MissingEnvVar(_)) => {
                "Set AZURE_CLIENT_ID, AZURE_CLIENT_SECRET, and AZURE_TENANT_ID environment variables"
            }
            ClientError::Forbidden { .. } => {
                "Your identity does not have permission to access this service.\n\
                 Assign an RBAC role on the resource, for example:\n\n\
                 \x20 az role assignment create \\\n\
                 \x20   --assignee <your-email-or-object-id> \\\n\
                 \x20   --role \"Search Service Contributor\" \\\n\
                 \x20   --scope /subscriptions/<sub>/resourceGroups/<rg>/providers/Microsoft.Search/searchServices/<name>\n\n\
                 Common roles: Search Service Contributor, Search Index Data Reader, Search Index Data Contributor.\n\
                 If using a service principal, ensure AZURE_TENANT_ID matches the service's tenant.\n\
                 See: https://learn.microsoft.com/en-us/azure/search/search-security-rbac"
            }
            ClientError::NotFound { .. } => {
                "Verify the resource name and that you have access to it"
            }
            ClientError::AlreadyExists { .. } => {
                "Use a different name or delete the existing resource first"
            }
            ClientError::RateLimited { .. } => "Wait and retry the operation",
            ClientError::ServiceUnavailable(_) => {
                "The Azure Search service may be temporarily unavailable. Try again later."
            }
            _ => "Check the error message for details",
        }
    }

    /// Get the raw response body (for error log details)
    pub fn raw_body(&self) -> Option<&str> {
        match self {
            ClientError::Forbidden { body, .. } => Some(body),
            ClientError::Api { message, .. } => Some(message),
            ClientError::ServiceUnavailable(body) => Some(body),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_response_azure_error_format() {
        let body = r#"{"error": {"message": "Index not found", "code": "ResourceNotFound"}}"#;
        let err = ClientError::from_response(404, body);
        match err {
            ClientError::Api { status, message } => {
                assert_eq!(status, 404);
                assert_eq!(message, "Index not found");
            }
            _ => panic!("Expected Api error"),
        }
    }

    #[test]
    fn test_from_response_plain_text() {
        let body = "Something went wrong";
        let err = ClientError::from_response(500, body);
        match err {
            ClientError::Api { status, message } => {
                assert_eq!(status, 500);
                assert_eq!(message, "Something went wrong");
            }
            _ => panic!("Expected Api error"),
        }
    }

    #[test]
    fn test_from_response_403_creates_forbidden() {
        let body = r#"{"detail": "forbidden"}"#;
        let err = ClientError::from_response(403, body);
        match err {
            ClientError::Forbidden {
                service,
                message,
                body: raw,
            } => {
                assert_eq!(service, "unknown service");
                assert!(message.is_empty()); // no error.message key in body
                assert_eq!(raw, body);
            }
            _ => panic!("Expected Forbidden error, got {:?}", err),
        }
    }

    #[test]
    fn test_from_response_with_url_403_extracts_service() {
        let body = r#"{"error": {"message": "Access denied"}}"#;
        let err = ClientError::from_response_with_url(
            403,
            body,
            Some("https://irma-prod-aisearch.search.windows.net/indexes?api-version=2024-07-01"),
        );
        match err {
            ClientError::Forbidden {
                service,
                message,
                body: _,
            } => {
                assert_eq!(service, "irma-prod-aisearch.search.windows.net");
                assert_eq!(message, "Access denied");
            }
            _ => panic!("Expected Forbidden error, got {:?}", err),
        }
    }

    #[test]
    fn test_from_response_with_url_403_empty_body() {
        let err = ClientError::from_response_with_url(
            403,
            "",
            Some("https://my-svc.search.windows.net/indexes?api-version=2024-07-01"),
        );
        match err {
            ClientError::Forbidden {
                service,
                message,
                body,
            } => {
                assert_eq!(service, "my-svc.search.windows.net");
                assert!(message.is_empty());
                assert!(body.is_empty());
            }
            _ => panic!("Expected Forbidden error, got {:?}", err),
        }
    }

    #[test]
    fn test_from_response_empty_body_fallback() {
        let err = ClientError::from_response(500, "  ");
        match err {
            ClientError::Api { status, message } => {
                assert_eq!(status, 500);
                assert!(message.contains("HTTP 500"));
                assert!(message.contains("no error details"));
            }
            _ => panic!("Expected Api error"),
        }
    }

    #[test]
    fn test_suggestion_forbidden() {
        let err = ClientError::Forbidden {
            service: "my-svc.search.windows.net".to_string(),
            message: "".to_string(),
            body: "".to_string(),
        };
        let suggestion = err.suggestion();
        assert!(suggestion.contains("permission"));
        assert!(suggestion.contains("Search Service Contributor"));
        assert!(suggestion.contains("az role assignment create"));
    }

    #[test]
    fn test_raw_body_forbidden() {
        let err = ClientError::Forbidden {
            service: "svc".to_string(),
            message: "".to_string(),
            body: "raw error body".to_string(),
        };
        assert_eq!(err.raw_body(), Some("raw error body"));
    }

    #[test]
    fn test_raw_body_api() {
        let err = ClientError::Api {
            status: 400,
            message: "bad request".to_string(),
        };
        assert_eq!(err.raw_body(), Some("bad request"));
    }

    #[test]
    fn test_raw_body_not_found_returns_none() {
        let err = ClientError::NotFound {
            kind: "Index".to_string(),
            name: "x".to_string(),
        };
        assert_eq!(err.raw_body(), None);
    }

    #[test]
    fn test_forbidden_display() {
        let err = ClientError::Forbidden {
            service: "my-svc.search.windows.net".to_string(),
            message: "".to_string(),
            body: "".to_string(),
        };
        let display = format!("{}", err);
        assert!(display.contains("403 Forbidden"));
        assert!(display.contains("my-svc.search.windows.net"));
    }

    #[test]
    fn test_is_retryable_rate_limited() {
        let err = ClientError::RateLimited { retry_after: 30 };
        assert!(err.is_retryable());
    }

    #[test]
    fn test_is_retryable_service_unavailable() {
        let err = ClientError::ServiceUnavailable("down".to_string());
        assert!(err.is_retryable());
    }

    #[test]
    fn test_is_not_retryable_api_error() {
        let err = ClientError::Api {
            status: 400,
            message: "bad request".to_string(),
        };
        assert!(!err.is_retryable());
    }

    #[test]
    fn test_is_not_retryable_not_found() {
        let err = ClientError::NotFound {
            kind: "Index".to_string(),
            name: "missing".to_string(),
        };
        assert!(!err.is_retryable());
    }

    #[test]
    fn test_suggestion_not_logged_in() {
        let err = ClientError::Auth(AuthError::NotLoggedIn);
        assert!(err.suggestion().contains("az login"));
    }

    #[test]
    fn test_suggestion_cli_not_found() {
        let err = ClientError::Auth(AuthError::AzCliNotFound);
        assert!(err.suggestion().contains("Install"));
    }

    #[test]
    fn test_suggestion_not_found() {
        let err = ClientError::NotFound {
            kind: "Index".to_string(),
            name: "x".to_string(),
        };
        assert!(err.suggestion().contains("Verify"));
    }

    #[test]
    fn test_suggestion_rate_limited() {
        let err = ClientError::RateLimited { retry_after: 60 };
        assert!(err.suggestion().contains("retry"));
    }
}
