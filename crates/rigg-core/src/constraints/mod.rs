//! Constraint validation for Azure AI Search resources

pub mod dependencies;
pub mod immutability;

pub use dependencies::{DependencyViolation, check_dependencies};
pub use immutability::{ImmutabilityViolation, ViolationSeverity, check_immutability};

use thiserror::Error;

/// Constraint violation errors
#[derive(Debug, Error)]
pub enum ConstraintError {
    #[error("Immutability violation: {0}")]
    Immutability(#[from] ImmutabilityViolation),
    #[error("Dependency violation: {0}")]
    Dependency(#[from] DependencyViolation),
}

/// Result of constraint validation
#[derive(Debug)]
pub struct ValidationResult {
    pub errors: Vec<ConstraintError>,
    pub warnings: Vec<String>,
}

impl ValidationResult {
    pub fn new() -> Self {
        Self {
            errors: Vec::new(),
            warnings: Vec::new(),
        }
    }

    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    pub fn add_error(&mut self, error: ConstraintError) {
        self.errors.push(error);
    }

    pub fn add_warning(&mut self, warning: String) {
        self.warnings.push(warning);
    }
}

impl Default for ValidationResult {
    fn default() -> Self {
        Self::new()
    }
}
