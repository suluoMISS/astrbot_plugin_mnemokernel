//! Read-only projections for the authenticated AstrBot administrator page.
use super::{KernelStore, StoreError};
use rusqlite::{params, types::ValueRef};
use serde::Deserialize;
use serde_json::{Map, Value, json};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectRequest {
    pub collection: String,
    #[serde(default)]
    pub scope_id: String,
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub offset: u32,
    pub limit: u32,
}

impl KernelStore {
    pub fn inspect(&self, request: &InspectRequest) -> Result<Value, StoreError> {
        if !(1..=50).contains(&request.limit)
            || request.offset > 100_000
            || request.query.chars().count() > 100
            || (request.collection != "scopes"
                && (request.scope_id.len() != 64
                    || !request.scope_id.bytes().all(|c| c.is_ascii_hexdigit())))
        {
            return Err(StoreError::Sqlite(rusqlite::Error::InvalidQuery));
        }
        // Only these fixed projections can reach SQLite. Never accept SQL,
        // column names or filesystem paths from the web request.
        let (source, search) = match request.collection.as_str() {
            "scopes" => (
                "SELECT s.scope_id, s.platform_id, s.bot_account_id, s.conversation_kind,
                 s.session_id, s.persona_id, s.created_at_ms AS time_ms,
                 COALESCE(p.policy_state, 'default') AS state
                 FROM scopes s LEFT JOIN scope_capture_policies p ON p.scope_id=s.scope_id",
                "platform_id || ' ' || session_id || ' ' || persona_id",
            ),
            "memories" => (
                "SELECT m.scope_id, m.memory_id AS id, m.memory_kind AS kind, m.state,
                 m.confidence, m.activation, m.updated_at_ms AS time_ms,
                 v.statement AS content FROM memory_atoms m JOIN memory_versions v
                 ON v.memory_id=m.memory_id AND v.version=m.current_version",
                "content",
            ),
            "journals" => (
                "SELECT e.scope_id, e.episode_id AS id, e.state,
                 e.occurred_from_ms AS time_ms, v.title, v.summary AS content,
                 v.open_loops_json, v.version FROM episodes e JOIN episode_versions v
                 ON v.episode_id=e.episode_id AND v.version=e.current_version",
                "title || ' ' || content",
            ),
            "events" => (
                "SELECT e.scope_id, e.event_id AS id, e.occurred_at_ms AS time_ms,
                 e.sender_is_bot, e.reply_to_source_key,
                 p.payload_state AS state, p.sender_display_name AS sender,
                 CASE WHEN p.payload_state IN ('active','redacted') THEN p.content
                 ELSE NULL END AS content FROM raw_events e JOIN raw_event_payloads p
                 ON p.event_id=e.event_id",
                "COALESCE(content, '')",
            ),
            "recalls" => (
                "SELECT scope_id, recall_id AS id, created_at_ms AS time_ms,
                 depth, decision AS state, reason AS content, decision_reason
                 FROM recall_audits",
                "content || ' ' || decision_reason",
            ),
            _ => return Err(StoreError::Sqlite(rusqlite::Error::InvalidQuery)),
        };
        let scope_filter = if request.collection == "scopes" {
            "1=1"
        } else {
            "scope_id=?1"
        };
        let filter = format!("{scope_filter} AND (?2='' OR instr(lower({search}),lower(?2))>0)");
        let connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let count: i64 = connection.query_row(
            &format!("SELECT COUNT(*) FROM ({source}) WHERE {filter}"),
            params![request.scope_id, request.query],
            |row| row.get(0),
        )?;
        let order = if request.collection == "scopes" {
            "scope_id"
        } else {
            "id"
        };
        let mut statement = connection.prepare(&format!(
            "SELECT * FROM ({source}) WHERE {filter} ORDER BY time_ms DESC, {order} LIMIT ?3 OFFSET ?4"
        ))?;
        let names: Vec<String> = statement
            .column_names()
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let rows = statement
            .query_map(
                params![
                    request.scope_id,
                    request.query,
                    request.limit,
                    request.offset
                ],
                |row| {
                    let mut object = Map::new();
                    for (index, name) in names.iter().enumerate() {
                        let value = match row.get_ref(index)? {
                            ValueRef::Null => Value::Null,
                            ValueRef::Integer(v) => json!(v),
                            ValueRef::Real(v) => json!(v),
                            ValueRef::Text(v) => json!(String::from_utf8_lossy(v)),
                            ValueRef::Blob(_) => Value::Null,
                        };
                        object.insert(name.clone(), value);
                    }
                    Ok(Value::Object(object))
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(json!({"items": rows, "total": count, "offset": request.offset, "limit": request.limit}))
    }
}
