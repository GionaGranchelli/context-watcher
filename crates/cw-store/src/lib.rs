use chrono::{DateTime, Utc};
use cw_core::{
    autopilot::{AmbientSavingsPulse, AutoPilotConfig},
    ir::{
        ContextTurn, FileContextState, FirewallAction, FirewallRule, FirewallTrigger, HarnessKind,
        MutationRule, MutationStatus, ProviderKind, Session,
    },
};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use std::collections::HashMap;
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
            );

            CREATE TABLE IF NOT EXISTS session_sources (
                session_id TEXT PRIMARY KEY,
                transcript_path TEXT NOT NULL,
                last_byte_offset INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS ambient_pulses (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                turn_seq INTEGER NOT NULL,
                tokens_saved INTEGER NOT NULL,
                cumulative_saved INTEGER NOT NULL,
                speedup_factor REAL NOT NULL,
                cost_saved REAL NOT NULL,
                pulse_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_pulses_session ON ambient_pulses(session_id);

            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value_json TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS mutation_effectiveness (
                mutation_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                turn_id TEXT NOT NULL,
                tokens_saved INTEGER NOT NULL,
                kv_cache_hit_rate REAL NOT NULL,
                ttft_ms INTEGER,
                created_at TEXT NOT NULL,
                FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
            );",
        )?;

        // Ensure any sessions originating from ~/.omp/ are properly categorized as OMP
        let _ = conn.execute(
            "UPDATE sessions
             SET harness = 'omp'
             WHERE harness = 'pi'
               AND id IN (
                   SELECT session_id FROM session_sources WHERE transcript_path LIKE '%/.omp/%'
               )",
            [],
        );

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
                "SELECT COALESCE(raw_request_hash, mutated_request_hash) FROM turns WHERE id = ?1",
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

    pub fn get_turn(&self, turn_id: &str) -> Result<Option<ContextTurn>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        let raw: Option<String> = conn
            .query_row(
                "SELECT turn_json FROM turns WHERE id = ?1",
                params![turn_id],
                |row| row.get(0),
            )
            .optional()?;
        match raw {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
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
        let count = conn.execute("DELETE FROM mutations WHERE id = ?1", params![mutation_id])?;
        Ok(count > 0)
    }

    pub fn update_mutation_status(
        &self,
        mutation_id: &str,
        status: MutationStatus,
    ) -> Result<bool, StoreError> {
        let status_str = match status {
            MutationStatus::Draft => "draft",
            MutationStatus::Committed => "committed",
            MutationStatus::Disabled => "disabled",
        };
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        let count = conn.execute(
            "UPDATE mutations SET status = ?1 WHERE id = ?2",
            params![status_str, mutation_id],
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

    pub fn set_session_source_path(
        &self,
        session_id: &str,
        transcript_path: &str,
        last_byte_offset: u64,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        conn.execute(
            "INSERT OR REPLACE INTO session_sources (session_id, transcript_path, last_byte_offset, updated_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                session_id,
                transcript_path,
                last_byte_offset as i64,
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn get_session_source_path(
        &self,
        session_id: &str,
    ) -> Result<Option<(String, u64)>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        let row: Option<(String, i64)> = conn
            .query_row(
                "SELECT transcript_path, last_byte_offset FROM session_sources WHERE session_id = ?1",
                params![session_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(row.map(|(p, off)| (p, off.max(0) as u64)))
    }

    pub fn list_session_sources(&self) -> Result<HashMap<String, String>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        let mut stmt = conn.prepare("SELECT session_id, transcript_path FROM session_sources")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let mut map = HashMap::new();
        for r in rows.flatten() {
            map.insert(r.0, r.1);
        }
        Ok(map)
    }

    pub fn record_ambient_pulse(&self, pulse: &AmbientSavingsPulse) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        let id = format!("{}_{}", pulse.session_id, pulse.turn_seq);
        let pulse_json = serde_json::to_string(pulse)?;
        conn.execute(
            "INSERT OR REPLACE INTO ambient_pulses (id, session_id, turn_seq, tokens_saved, cumulative_saved, speedup_factor, cost_saved, pulse_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                id,
                pulse.session_id,
                pulse.turn_seq,
                pulse.tokens_saved_this_pulse as i64,
                pulse.cumulative_tokens_saved as i64,
                pulse.speedup_factor as f64,
                pulse.estimated_cost_saved_usd,
                pulse_json,
                pulse.timestamp.to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn get_latest_pulse_for_session(
        &self,
        session_id: &str,
    ) -> Result<Option<AmbientSavingsPulse>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        let raw: Option<String> = conn
            .query_row(
                "SELECT pulse_json FROM ambient_pulses WHERE session_id = ?1 ORDER BY turn_seq DESC, created_at DESC LIMIT 1",
                params![session_id],
                |row| row.get(0),
            )
            .optional()?;
        match raw {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    pub fn list_recent_ambient_pulses(
        &self,
        limit: usize,
    ) -> Result<Vec<AmbientSavingsPulse>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        let mut stmt = conn
            .prepare("SELECT pulse_json FROM ambient_pulses ORDER BY created_at DESC LIMIT ?1")?;
        let rows = stmt.query_map(params![limit as i64], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            let s = r?;
            out.push(serde_json::from_str(&s)?);
        }
        Ok(out)
    }

    pub fn get_autopilot_config(&self) -> Result<AutoPilotConfig, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        let raw: Option<String> = conn
            .query_row(
                "SELECT value_json FROM settings WHERE key = 'autopilot_config'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        match raw {
            Some(s) => Ok(serde_json::from_str(&s).unwrap_or_default()),
            None => Ok(AutoPilotConfig::default()),
        }
    }

    pub fn set_autopilot_config(&self, cfg: &AutoPilotConfig) -> Result<(), StoreError> {
        let json_str = serde_json::to_string(cfg)?;
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value_json) VALUES ('autopilot_config', ?1)",
            params![json_str],
        )?;
        Ok(())
    }

    pub fn record_mutation_effectiveness(
        &self,
        record: &MutationEffectivenessRecord,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        conn.execute(
            "INSERT OR REPLACE INTO mutation_effectiveness (
                mutation_id, session_id, turn_id, tokens_saved, kv_cache_hit_rate, ttft_ms, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                record.mutation_id,
                record.session_id,
                record.turn_id,
                record.tokens_saved,
                record.kv_cache_hit_rate,
                record.ttft_ms.map(|v| v as i64),
                record.created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn list_mutation_effectiveness(
        &self,
        session_id: Option<&str>,
    ) -> Result<Vec<MutationEffectivenessRecord>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        let mut res = Vec::new();
        if let Some(sid) = session_id {
            let mut stmt = conn.prepare(
                "SELECT mutation_id, session_id, turn_id, tokens_saved, kv_cache_hit_rate, ttft_ms, created_at
                 FROM mutation_effectiveness WHERE session_id = ?1 ORDER BY created_at DESC"
            )?;
            let rows = stmt.query_map(params![sid], Self::row_to_effectiveness)?;
            for r in rows.flatten() {
                res.push(r);
            }
        } else {
            let mut stmt = conn.prepare(
                "SELECT mutation_id, session_id, turn_id, tokens_saved, kv_cache_hit_rate, ttft_ms, created_at
                 FROM mutation_effectiveness ORDER BY created_at DESC LIMIT 100"
            )?;
            let rows = stmt.query_map([], Self::row_to_effectiveness)?;
            for r in rows.flatten() {
                res.push(r);
            }
        }
        Ok(res)
    }

    fn row_to_effectiveness(row: &rusqlite::Row) -> rusqlite::Result<MutationEffectivenessRecord> {
        let created_str: String = row.get(6)?;
        let created_at = DateTime::parse_from_rfc3339(&created_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        let ttft_raw: Option<i64> = row.get(5)?;

        Ok(MutationEffectivenessRecord {
            mutation_id: row.get(0)?,
            session_id: row.get(1)?,
            turn_id: row.get(2)?,
            tokens_saved: row.get(3)?,
            kv_cache_hit_rate: row.get(4)?,
            ttft_ms: ttft_raw.map(|v| v as u64),
            created_at,
        })
    }
}

pub use cw_core::ir::MutationEffectivenessRecord;
use cw_core::repository::{ContextRepository, MutationRepository, RepositoryError};

impl From<StoreError> for RepositoryError {
    fn from(e: StoreError) -> Self {
        RepositoryError::Storage(e.to_string())
    }
}

impl ContextRepository for CwStore {
    fn upsert_session(&self, session: &Session) -> Result<(), RepositoryError> {
        self.upsert_session(session).map_err(Into::into)
    }

    fn get_session(&self, session_id: &str) -> Result<Option<Session>, RepositoryError> {
        self.get_session(session_id).map_err(Into::into)
    }

    fn list_sessions(&self) -> Result<Vec<Session>, RepositoryError> {
        self.list_sessions().map_err(Into::into)
    }

    fn save_turn(
        &self,
        turn: &ContextTurn,
        raw_payload: Option<&Value>,
        mutated_payload: Option<&Value>,
        raw_response: Option<&Value>,
    ) -> Result<(), RepositoryError> {
        self.save_turn(turn, raw_payload, mutated_payload, raw_response)
            .map_err(Into::into)
    }

    fn get_latest_turn(&self, session_id: &str) -> Result<Option<ContextTurn>, RepositoryError> {
        self.get_latest_turn(session_id).map_err(Into::into)
    }

    fn get_turn(&self, turn_id: &str) -> Result<Option<ContextTurn>, RepositoryError> {
        self.get_turn(turn_id).map_err(Into::into)
    }

    fn list_turns(&self, session_id: &str) -> Result<Vec<ContextTurn>, RepositoryError> {
        self.list_turns(session_id).map_err(Into::into)
    }

    fn put_blob(&self, content: &Value) -> Result<String, RepositoryError> {
        self.put_blob(content).map_err(Into::into)
    }

    fn get_blob(&self, hash: &str) -> Result<Option<Value>, RepositoryError> {
        self.get_blob(hash).map_err(Into::into)
    }
}

impl MutationRepository for CwStore {
    fn save_mutation(&self, rule: &MutationRule) -> Result<(), RepositoryError> {
        self.save_mutation(rule).map_err(Into::into)
    }

    fn update_mutation_status(
        &self,
        mutation_id: &str,
        status: MutationStatus,
    ) -> Result<bool, RepositoryError> {
        self.update_mutation_status(mutation_id, status)
            .map_err(Into::into)
    }

    fn list_mutations(&self, session_id: &str) -> Result<Vec<MutationRule>, RepositoryError> {
        self.list_mutations(session_id).map_err(Into::into)
    }

    fn delete_mutation(&self, mutation_id: &str) -> Result<bool, RepositoryError> {
        self.delete_mutation(mutation_id).map_err(Into::into)
    }

    fn list_firewall_rules(
        &self,
        session_id: Option<&str>,
    ) -> Result<Vec<FirewallRule>, RepositoryError> {
        self.list_firewall_rules(session_id).map_err(Into::into)
    }

    fn record_mutation_effectiveness(
        &self,
        record: &MutationEffectivenessRecord,
    ) -> Result<(), RepositoryError> {
        self.record_mutation_effectiveness(record)
            .map_err(Into::into)
    }

    fn list_mutation_effectiveness(
        &self,
        session_id: Option<&str>,
    ) -> Result<Vec<MutationEffectivenessRecord>, RepositoryError> {
        self.list_mutation_effectiveness(session_id)
            .map_err(Into::into)
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

        store.save_turn(&turn, Some(&payload), None, None).unwrap();
        store.save_file_states("sess_1", &file_states).unwrap();

        let loaded = store.get_latest_turn("sess_1").unwrap().unwrap();
        assert_eq!(loaded.id, "turn_1");
        assert_eq!(loaded.sequence_number, 1);

        let raw = store.get_turn_raw_payload("turn_1").unwrap().unwrap();
        assert_eq!(raw["model"], "claude-sonnet-4");
    }

    #[test]
    fn test_sqlite_autopilot_and_mutations_and_effectiveness() {
        let store = CwStore::open_in_memory().unwrap();
        let session = Session {
            id: "sess_ap".into(),
            parent_session_id: None,
            harness: HarnessKind::Omp,
            workspace_root: Some("/test".into()),
            model: "flint".into(),
            provider: ProviderKind::OpenAICompatible,
            title: Some("AutoPilot Test".into()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        store.upsert_session(&session).unwrap();

        // 1. AutoPilot Config persistence
        let mut cfg = store.get_autopilot_config().unwrap();
        cfg.min_health_score = 82;
        cfg.heal_strategy = "mid".to_string();
        store.set_autopilot_config(&cfg).unwrap();
        let loaded_cfg = store.get_autopilot_config().unwrap();
        assert_eq!(loaded_cfg.min_health_score, 82);
        assert_eq!(loaded_cfg.heal_strategy, "mid");

        // 2. Mutations persistence
        let rule = cw_core::ir::MutationRule {
            id: "mut_test_1".into(),
            session_id: "sess_ap".into(),
            op: cw_core::ir::MutationOp::ShadowDedupe {
                file_path: Some("Cargo.toml".into()),
            },
            status: cw_core::ir::MutationStatus::Committed,
            estimated_saved_tokens: 150,
            estimated_kv_break_tokens: 0,
            created_at: Utc::now(),
        };
        store.save_mutation(&rule).unwrap();
        let muts = store.list_mutations("sess_ap").unwrap();
        assert_eq!(muts.len(), 1);
        assert_eq!(muts[0].id, "mut_test_1");
        assert_eq!(muts[0].estimated_saved_tokens, 150);

        // 3. Mutation effectiveness telemetry persistence
        let eff = MutationEffectivenessRecord {
            mutation_id: "mut_test_1".into(),
            session_id: "sess_ap".into(),
            turn_id: "turn_1".into(),
            tokens_saved: 150,
            kv_cache_hit_rate: 0.95,
            ttft_ms: Some(250),
            created_at: Utc::now(),
        };
        store.record_mutation_effectiveness(&eff).unwrap();
        let records = store.list_mutation_effectiveness(Some("sess_ap")).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].mutation_id, "mut_test_1");
        assert_eq!(records[0].ttft_ms, Some(250));

        // 4. Session source path persistence
        store
            .set_session_source_path("sess_ap", "/tmp/transcript.jsonl", 1024)
            .unwrap();
        let (src, len) = store.get_session_source_path("sess_ap").unwrap().unwrap();
        assert_eq!(src, "/tmp/transcript.jsonl");
        assert_eq!(len, 1024);

        // 5. Ambient pulse persistence
        let pulse = AmbientSavingsPulse {
            session_id: "sess_ap".into(),
            turn_seq: 1,
            tokens_saved_this_pulse: 1500,
            cumulative_tokens_saved: 1500,
            total_input_tokens: 3000,
            saved_percentage: 33.3,
            kv_cache_hit_ratio: 95.0,
            ttft_before_ms: 1200,
            ttft_after_ms: 300,
            speedup_factor: 4.0,
            estimated_cost_saved_usd: 0.05,
            is_local_model: false,
            formatted_message: "⚡ [cw] We saved you 1500 tokens (-33.3%) | KV Cache 95.0% preserved | TTFT: 1200ms -> 300ms (4.0x faster) | $0.05 saved".into(),
            timestamp: Utc::now(),
        };
        store.record_ambient_pulse(&pulse).unwrap();
        let fetched_pulse = store
            .get_latest_pulse_for_session("sess_ap")
            .unwrap()
            .unwrap();
        assert_eq!(fetched_pulse.tokens_saved_this_pulse, 1500);
        assert_eq!(fetched_pulse.speedup_factor, 4.0);
        let pulses = store.list_recent_ambient_pulses(10).unwrap();
        assert_eq!(pulses.len(), 1);
        assert!(
            pulses[0]
                .formatted_message
                .contains("⚡ [cw] We saved you 1500 tokens")
        );
    }
}
