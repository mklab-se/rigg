//! Azure AI Search resource definitions

pub mod agent;
pub mod alias;
pub mod datasource;
pub mod index;
pub mod indexer;
pub mod knowledge_base;
pub mod knowledge_source;
pub mod managed;
pub mod skillset;
pub mod synonym_map;
pub mod traits;

pub use alias::Alias;
pub use datasource::DataSource;
pub use index::Index;
pub use indexer::Indexer;
pub use knowledge_base::KnowledgeBase;
pub use knowledge_source::KnowledgeSource;
pub use skillset::Skillset;
pub use synonym_map::SynonymMap;
pub use traits::{Resource, ResourceKind, validate_resource_name};
