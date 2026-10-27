use crate::ir::{
    ContextTurn, FirewallRule, MutationEffectivenessRecord, MutationRule, MutationStatus, Session,
};
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum RepositoryError {
    #[error("Storage error: {0}")]
    Storage(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("Not found: {0}")]
    NotFound(String),
}

/// Trait defining context and session storage operations.
pub trait ContextRepository: Send + Sync {
    fn upsert_session(&self, session: &Session) -> Result<(), RepositoryError>;
    fn get_session(&self, session_id: &str) -> Result<Option<Session>, RepositoryError>;
    fn list_sessions(&self) -> Result<Vec<Session>, RepositoryError>;
    fn save_turn(
        &self,
        turn: &ContextTurn,
        raw_payload: Option<&Value>,
        mutated_payload: Option<&Value>,
        raw_response: Option<&Value>,
    ) -> Result<(), RepositoryError>;
    fn get_latest_turn(&self, session_id: &str) -> Result<Option<ContextTurn>, RepositoryError>;
    fn get_turn(&self, turn_id: &str) -> Result<Option<ContextTurn>, RepositoryError>;
    fn list_turns(&self, session_id: &str) -> Result<Vec<ContextTurn>, RepositoryError>;
    fn put_blob(&self, content: &Value) -> Result<String, RepositoryError>;
    fn get_blob(&self, hash: &str) -> Result<Option<Value>, RepositoryError>;
}

/// Trait defining active mutation and policy operations.
pub trait MutationRepository: Send + Sync {
    fn save_mutation(&self, rule: &MutationRule) -> Result<(), RepositoryError>;
    fn update_mutation_status(
        &self,
        mutation_id: &str,
        status: MutationStatus,
    ) -> Result<bool, RepositoryError>;
    fn list_mutations(&self, session_id: &str) -> Result<Vec<MutationRule>, RepositoryError>;
    fn delete_mutation(&self, mutation_id: &str) -> Result<bool, RepositoryError>;
    fn list_firewall_rules(
        &self,
        session_id: Option<&str>,
    ) -> Result<Vec<FirewallRule>, RepositoryError>;
    fn record_mutation_effectiveness(
        &self,
        record: &MutationEffectivenessRecord,
    ) -> Result<(), RepositoryError>;
    fn list_mutation_effectiveness(
        &self,
        session_id: Option<&str>,
    ) -> Result<Vec<MutationEffectivenessRecord>, RepositoryError>;
}
