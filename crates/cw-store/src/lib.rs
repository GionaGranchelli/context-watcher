use chrono::{DateTime, Utc};
use cw_core::ir::{
    ContextTurn, FileContextState, FirewallAction, FirewallRule, FirewallTrigger, HarnessKind,
    MutationRule, MutationStatus, ProviderKind, Session,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Lock poisoned")]
    LockPoisoned,
}

#[derive(Clone)]
pub struct CwStore {
    conn: Arc<Mutex<Connection>>,
}

impl CwStore {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, StoreError> {
        if let Some(parent) = path.as_ref().parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;",
        )?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        store.init_schema()?;
        store.seed_default_firewall_rules()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        store.init_schema()?;
        store.seed_default_firewall_rules()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS blobs (
                hash TEXT PRIMARY KEY,
                content_json TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                parent_session_id TEXT,
                harness TEXT NOT NULL,
                workspace_root TEXT,
                model TEXT NOT NULL,
                provider TEXT NOT NULL,
                title TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS turns (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                parent_turn_id TEXT,
                seq_num INTEGER NOT NULL,
                turn_json TEXT NOT NULL,
                raw_request_hash TEXT,
                mutated_request_hash TEXT,
                raw_response_hash TEXT,
                created_at TEXT NOT NULL,
                FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_turns_session_seq ON turns(session_id, seq_num);

            CREATE TABLE IF NOT EXISTS mutations (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                rule_json TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS firewall_rules (
                id TEXT PRIMARY KEY,
                session_id TEXT,
                rule_json TEXT NOT NULL,
                enabled INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS file_states (
                session_id TEXT PRIMARY KEY,
                states_json TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );",
        )?;
        Ok(())
    }

    fn seed_default_firewall_rules(&self) -> Result<(), StoreError> {
        let existing = self.list_firewall_rules(None)?;
        if !existing.is_empty() {
            return Ok(());
        }
        let default_rule = FirewallRule {
            id: "fw_default_giant_output".to_string(),
            session_id: None,
            name: "Auto-Truncate Giant Tool Outputs (>60k chars)".to_string(),
            trigger: FirewallTrigger::SingleToolResultTokensExceed { max_tokens: 15_000 },
            action: FirewallAction::AutoTruncateHeadTail {
                head_chars: 12_000,
                tail_chars: 8_000,
            },
            enabled: true,
        };
        self.upsert_firewall_rule(&default_rule)?;
        Ok(())
    }

    pub fn put_blob(&self, val: &Value) -> Result<String, StoreError> {
        let json_str = serde_json::to_string(val)?;
        let hash = blake3::hash(json_str.as_bytes()).to_hex().to_string();
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        conn.execute(
            "INSERT OR IGNORE INTO blobs (hash, content_json) VALUES (?1, ?2)",
            params![hash, json_str],
        )?;
        Ok(hash)
    }

    pub fn get_blob(&self, hash: &str) -> Result<Option<Value>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        let raw: Option<String> = conn
            .query_row(
                "SELECT content_json FROM blobs WHERE hash = ?1",
                params![hash],
                |row| row.get(0),
            )
            .optional()?;
        match raw {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    pub fn upsert_session(&self, session: &Session) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        conn.execute(
            "INSERT INTO sessions (id, parent_session_id, harness, workspace_root, model, provider, title, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET
                workspace_root = excluded.workspace_root,
                model = excluded.model,
                title = COALESCE(excluded.title, sessions.title),
                created_at = excluded.created_at,
                updated_at = excluded.updated_at",
            params![
                session.id,
                session.parent_session_id,
                session.harness.as_str(),
                session.workspace_root,
                session.model,
                session.provider.as_str(),
                session.title,
                session.created_at.to_rfc3339(),
                session.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn list_sessions(&self) -> Result<Vec<Session>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        let mut stmt = conn.prepare(
            "SELECT id, parent_session_id, harness, workspace_root, model, provider, title, created_at, updated_at
             FROM sessions ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            let created_str: String = row.get(7)?;
            let updated_str: String = row.get(8)?;
            Ok(Session {
                id: row.get(0)?,
                parent_session_id: row.get(1)?,
                harness: HarnessKind::from_str_lossy(&row.get::<_, String>(2)?),
                workspace_root: row.get(3)?,
                model: row.get(4)?,
                provider: ProviderKind::from_str_lossy(&row.get::<_, String>(5)?),
                title: row.get(6)?,
                created_at: DateTime::parse_from_rfc3339(&created_str)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                updated_at: DateTime::parse_from_rfc3339(&updated_str)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn get_session(&self, session_id: &str) -> Result<Option<Session>, StoreError> {
        let sessions = self.list_sessions()?;
        Ok(sessions.into_iter().find(|s| s.id == session_id))
    }

    pub fn save_turn(
        &self,
        turn: &ContextTurn,
        raw_req: Option<&Value>,
        mutated_req: Option<&Value>,
        raw_resp: Option<&Value>,
    ) -> Result<(), StoreError> {
        let raw_req_hash = match raw_req {
            Some(v) => Some(self.put_blob(v)?),
            None => None,
        };
        let mut_req_hash = match mutated_req {
            Some(v) => Some(self.put_blob(v)?),
            None => None,
        };
        let raw_resp_hash = match raw_resp {
            Some(v) => Some(self.put_blob(v)?),
            None => None,
        };

        let turn_json = serde_json::to_string(turn)?;
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        conn.execute(
            "INSERT OR REPLACE INTO turns (id, session_id, parent_turn_id, seq_num, turn_json, raw_request_hash, mutated_request_hash, raw_response_hash, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                turn.id,
                turn.session_id,
                turn.parent_turn_id,
                turn.sequence_number,
                turn_json,
                raw_req_hash,
                mut_req_hash,
                raw_resp_hash,
                turn.created_at.to_rfc3339(),
            ],
        )?;
        conn.execute(
            "UPDATE sessions SET updated_at = ?1 WHERE id = ?2",
            params![turn.created_at.to_rfc3339(), turn.session_id],
        )?;
        Ok(())
    }

    pub fn list_turns(&self, session_id: &str) -> Result<Vec<ContextTurn>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        let mut stmt = conn.prepare(
            "SELECT turn_json FROM turns WHERE session_id = ?1 ORDER BY seq_num ASC, created_at ASC",
        )?;
        let rows = stmt.query_map(params![session_id], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            let s = r?;
            out.push(serde_json::from_str(&s)?);
        }
        Ok(out)
    }

    pub fn get_latest_turn(&self, session_id: &str) -> Result<Option<ContextTurn>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        let raw: Option<String> = conn
            .query_row(
                "SELECT turn_json FROM turns WHERE session_id = ?1 ORDER BY seq_num DESC, created_at DESC LIMIT 1",
                params![session_id],
                |row| row.get(0),
            )
            .optional()?;
        match raw {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    pub fn get_turn_raw_payload(&self, turn_id: &str) -> Result<Option<Value>, StoreError> {
        let hash_opt: Option<String> = {
            let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
            conn.query_row(
                "SELECT COALESCE(mutated_request_hash, raw_request_hash) FROM turns WHERE id = ?1",
                params![turn_id],
                |row| row.get(0),
            )
            .optional()?
            .flatten()
        };
        match hash_opt {
            Some(h) => self.get_blob(&h),
            None => Ok(None),
        }
    }

    pub fn save_mutation(&self, rule: &MutationRule) -> Result<(), StoreError> {
        let json_str = serde_json::to_string(rule)?;
        let status_str = match rule.status {
            MutationStatus::Draft => "draft",
            MutationStatus::Committed => "committed",
            MutationStatus::Disabled => "disabled",
        };
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        conn.execute(
            "INSERT OR REPLACE INTO mutations (id, session_id, rule_json, status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                rule.id,
                rule.session_id,
                json_str,
                status_str,
                rule.created_at.to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn list_mutations(&self, session_id: &str) -> Result<Vec<MutationRule>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        let mut stmt = conn.prepare(
            "SELECT rule_json FROM mutations WHERE session_id = ?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map(params![session_id], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(serde_json::from_str(&r?)?);
        }
        Ok(out)
    }

    pub fn delete_mutation(&self, mutation_id: &str) -> Result<bool, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        let count = conn.execute(
            "DELETE FROM mutations WHERE id = ?1",
            params![mutation_id],
        )?;
        Ok(count > 0)
    }

    pub fn commit_all_draft_mutations(&self, session_id: &str) -> Result<usize, StoreError> {
        let mut rules = self.list_mutations(session_id)?;
        let mut committed_count = 0;
        for rule in &mut rules {
            if rule.status == MutationStatus::Draft {
                rule.status = MutationStatus::Committed;
                self.save_mutation(rule)?;
                committed_count += 1;
            }
        }
        Ok(committed_count)
    }

    pub fn save_file_states(
        &self,
        session_id: &str,
        states: &[FileContextState],
    ) -> Result<(), StoreError> {
        let json_str = serde_json::to_string(states)?;
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        conn.execute(
            "INSERT OR REPLACE INTO file_states (session_id, states_json, updated_at)
             VALUES (?1, ?2, ?3)",
            params![session_id, json_str, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn get_file_states(&self, session_id: &str) -> Result<Vec<FileContextState>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        let raw: Option<String> = conn
            .query_row(
                "SELECT states_json FROM file_states WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .optional()?;
        match raw {
            Some(s) => Ok(serde_json::from_str(&s)?),
            None => Ok(Vec::new()),
        }
    }

    pub fn upsert_firewall_rule(&self, rule: &FirewallRule) -> Result<(), StoreError> {
        let json_str = serde_json::to_string(rule)?;
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        conn.execute(
            "INSERT OR REPLACE INTO firewall_rules (id, session_id, rule_json, enabled)
             VALUES (?1, ?2, ?3, ?4)",
            params![rule.id, rule.session_id, json_str, rule.enabled as i32],
        )?;
        Ok(())
    }

    pub fn list_firewall_rules(
        &self,
        session_id: Option<&str>,
    ) -> Result<Vec<FirewallRule>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        let mut stmt = conn.prepare(
            "SELECT rule_json FROM firewall_rules WHERE session_id IS NULL OR session_id = ?1",
        )?;
        let rows = stmt.query_map(params![session_id], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(serde_json::from_str(&r?)?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cw_core::analyze_turn_from_payload;
    use serde_json::json;

    #[test]
    fn test_sqlite_store_roundtrip() {
        let store = CwStore::open_in_memory().unwrap();
        let session = Session {
            id: "sess_1".into(),
            parent_session_id: None,
            harness: HarnessKind::Antigravity,
            workspace_root: Some("/workspace".into()),
            model: "claude-sonnet-4".into(),
            provider: ProviderKind::Anthropic,
            title: Some("Test Session".into()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        store.upsert_session(&session).unwrap();

        let payload = json!({
            "model": "claude-sonnet-4",
            "max_tokens": 1024,
            "messages": [{ "role": "user", "content": "Hello" }]
        });
        let (turn, file_states) = analyze_turn_from_payload(
            "turn_1".into(),
            "sess_1".into(),
            None,
            1,
            &payload,
            ProviderKind::Anthropic,
            None,
            Some(12),
            None,
        )
        .unwrap();

        store
            .save_turn(&turn, Some(&payload), None, None)
            .unwrap();
        store.save_file_states("sess_1", &file_states).unwrap();

        let loaded = store.get_latest_turn("sess_1").unwrap().unwrap();
        assert_eq!(loaded.id, "turn_1");
        assert_eq!(loaded.sequence_number, 1);

        let raw = store.get_turn_raw_payload("turn_1").unwrap().unwrap();
        assert_eq!(raw["model"], "claude-sonnet-4");
    }
}
