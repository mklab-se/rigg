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
    /// Create an API error from HTTP status and response body
    pub fn from_response(status: u16, body: &str) -> Self {
        // Try to parse Azure error format
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
            if let Some(error) = json.get("error") {
                let message = error
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or(body)
                    .to_string();
                return Self::Api { status, message };
            }
        }

        Self::Api {
            status,
            message: body.to_string(),
        }
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
    fn test_from_response_json_without_error_key() {
        let body = r#"{"detail": "forbidden"}"#;
        let err = ClientError::from_response(403, body);
        match err {
            ClientError::Api { status, message } => {
                assert_eq!(status, 403);
                assert_eq!(message, body);
            }
            _ => panic!("Expected Api error"),
        }
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
