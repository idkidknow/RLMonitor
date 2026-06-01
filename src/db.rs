use crate::model::{
    Activity, ChatEvent, EventType, FilterKind, MessagesPage, MessagesQuery, Stats,
};
use anyhow::{Context, Result, anyhow, ensure};
use chrono::{Duration, Timelike, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use tokio::sync::Semaphore;

#[derive(Clone)]
pub struct Database(Arc<Mutex<Connection>>, Arc<Semaphore>);

#[derive(Debug)]
pub struct CursorExpired;
impl std::fmt::Display for CursorExpired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Cursor expired or unknown")
    }
}
impl std::error::Error for CursorExpired {}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        let mut conn = Connection::open(path).context("Cannot open SQLite database")?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
        migrate(&mut conn)?;
        Ok(Self(
            Arc::new(Mutex::new(conn)),
            Arc::new(Semaphore::new(1)),
        ))
    }

    async fn run<T, F>(&self, work: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    {
        let db = self.0.clone();
        // Queue asynchronously before entering the blocking pool. The worker owns
        // the permit so cancelling an HTTP request cannot release it prematurely.
        let permit = self
            .1
            .clone()
            .acquire_owned()
            .await
            .context("Database queue closed")?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut conn = db.lock().map_err(|_| anyhow!("Database lock poisoned"))?;
            work(&mut conn)
        })
        .await
        .context("Database worker failed")?
    }

    pub async fn insert(
        &self,
        kind: EventType,
        content: Option<String>,
        raw: Option<String>,
    ) -> Result<ChatEvent> {
        self.run(move |conn| {
            let ts = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            Ok(conn.query_row(
                "INSERT INTO events (id, timestamp, event_type, content, raw_json)
                 VALUES (?1 || '_' || printf('%012d', COALESCE((SELECT seq FROM sqlite_sequence WHERE name = 'events'), 0) + 1), ?1, ?2, ?3, ?4)
                 RETURNING id, timestamp, event_type, content, raw_json",
                params![ts, kind.as_str(), content, raw], row_to_event,
            )?)
        }).await
    }

    pub async fn query(&self, query: MessagesQuery) -> Result<MessagesPage> {
        self.run(move |conn| query_events(conn, &query)).await
    }

    pub async fn stats(&self) -> Result<Stats> {
        self.run(|conn| {
            let now = Utc::now();
            let cutoff = (now - Duration::hours(24)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            let (stored_messages, messages_24h, last_message_at) = conn.query_row(
                "SELECT count(*), COALESCE(sum(timestamp >= ?1), 0), max(timestamp) FROM events WHERE event_type = 'chat'",
                [cutoff], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            let this_hour = now.with_minute(0).and_then(|d| d.with_second(0)).and_then(|d| d.with_nanosecond(0)).context("Invalid hour")?;
            let mut statement = conn.prepare("SELECT strftime('%Y-%m-%dT%H:00:00Z', timestamp), count(*) FROM events WHERE event_type = 'chat' AND timestamp >= ?1 GROUP BY 1")?;
            let start = this_hour - Duration::hours(23);
            let counts = statement.query_map([start.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?.collect::<rusqlite::Result<std::collections::HashMap<_, _>>>()?;
            let activity = (0..24).map(|i| {
                let hour = (start + Duration::hours(i)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
                Activity { count: *counts.get(&hour).unwrap_or(&0), hour }
            }).collect();
            Ok(Stats { stored_messages, messages_24h, last_message_at, activity })
        }).await
    }

    pub async fn cleanup(&self, days: u32) -> Result<usize> {
        self.run(move |conn| {
            let cutoff = (Utc::now() - Duration::days(days.into()))
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            Ok(conn.execute("DELETE FROM events WHERE timestamp < ?1", [cutoff])?)
        })
        .await
    }
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let version: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    ensure!(
        version <= 1,
        "Database schema is newer than this application"
    );
    if version == 1 {
        return Ok(());
    }
    let transaction = conn.transaction()?;
    let old_exists = transaction
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='events'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    transaction.execute_batch(
        "CREATE TABLE events_new (
        sequence INTEGER PRIMARY KEY AUTOINCREMENT,
        id TEXT NOT NULL UNIQUE,
        timestamp TEXT NOT NULL,
        event_type TEXT NOT NULL CHECK (event_type IN ('chat', 'connected', 'disconnected')),
        content TEXT, raw_json TEXT
    );",
    )?;
    if old_exists {
        transaction.execute_batch("INSERT INTO events_new (id, timestamp, event_type, content, raw_json) SELECT id, timestamp, event_type, content, raw_json FROM events ORDER BY timestamp, id; DROP TABLE events;")?;
    }
    transaction.execute_batch(
        "ALTER TABLE events_new RENAME TO events;
        CREATE INDEX idx_events_timestamp ON events(timestamp);
        CREATE INDEX idx_events_type_sequence ON events(event_type, sequence);
        PRAGMA user_version = 1;",
    )?;
    transaction.commit()?;
    Ok(())
}

fn query_events(conn: &Connection, query: &MessagesQuery) -> Result<MessagesPage> {
    let cursor = query.before.as_ref().or(query.after.as_ref());
    let sequence = match cursor {
        Some(id) => Some(
            conn.query_row("SELECT sequence FROM events WHERE id = ?1", [id], |r| {
                r.get::<_, i64>(0)
            })
            .optional()?
            .ok_or(CursorExpired)?,
        ),
        None => None,
    };
    let limit = query.limit.unwrap_or(50);
    let kind = match query.kind {
        FilterKind::All => "all",
        FilterKind::Chat => "chat",
        FilterKind::Connections => "connections",
    };
    let q = query.q.as_deref().unwrap_or("");
    let direction = if query.after.is_some() { "ASC" } else { "DESC" };
    let mut stmt = conn.prepare(&format!(
        "SELECT id, timestamp, event_type, content, raw_json FROM events
         WHERE (?1 IS NULL OR (?2 AND sequence > ?1) OR (NOT ?2 AND sequence < ?1))
         AND (?3 = 'all' OR (?3 = 'chat' AND event_type = 'chat') OR (?3 = 'connections' AND event_type != 'chat'))
         AND (?4 = '' OR instr(lower(COALESCE(content, '')), lower(?4)) > 0)
         ORDER BY sequence {direction} LIMIT ?5"
    ))?;
    let mut events = stmt
        .query_map(
            params![sequence, query.after.is_some(), kind, q, limit + 1],
            row_to_event,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let has_more = events.len() > limit as usize;
    events.truncate(limit as usize);
    if query.after.is_none() {
        events.reverse();
    }
    Ok(MessagesPage { events, has_more })
}

fn row_to_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChatEvent> {
    let value: String = row.get(2)?;
    let event_type = match value.as_str() {
        "chat" => EventType::Chat,
        "connected" => EventType::Connected,
        "disconnected" => EventType::Disconnected,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    Ok(ChatEvent {
        id: row.get(0)?,
        timestamp: row.get(1)?,
        event_type,
        content: row.get(3)?,
        raw_json: row.get(4)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn memory() -> Database {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        Database(Arc::new(Mutex::new(conn)), Arc::new(Semaphore::new(1)))
    }
    #[test]
    fn migrates_existing_history_without_changing_ids() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE events (id TEXT PRIMARY KEY, timestamp TEXT NOT NULL, event_type TEXT NOT NULL, content TEXT, raw_json TEXT);
            INSERT INTO events VALUES ('old_0001', '2026-01-01T00:00:00.000Z', 'chat', 'hello', '{}');").unwrap();
        migrate(&mut conn).unwrap();
        migrate(&mut conn).unwrap();
        let page = query_events(&conn, &MessagesQuery::default()).unwrap();
        assert_eq!(page.events[0].id, "old_0001");
        assert_eq!(page.events[0].content.as_deref(), Some("hello"));
    }
    #[tokio::test]
    async fn pagination_search_and_recovery_are_stable() {
        let db = memory();
        let first = db
            .insert(EventType::Chat, Some("100% diamonds".into()), None)
            .await
            .unwrap();
        let second = db.insert(EventType::Connected, None, None).await.unwrap();
        let third = db
            .insert(EventType::Chat, Some("HELLO Alex".into()), None)
            .await
            .unwrap();
        assert_ne!(first.id, second.id);
        let page = db
            .query(MessagesQuery {
                limit: Some(2),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(page.has_more);
        assert_eq!(page.events[0].id, second.id);
        let previous = db
            .query(MessagesQuery {
                before: Some(second.id.clone()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(previous.events[0].id, first.id);
        let missed = db
            .query(MessagesQuery {
                after: Some(first.id),
                limit: Some(1),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(missed.has_more);
        assert_eq!(missed.events[0].id, second.id);
        let literal = db
            .query(MessagesQuery {
                q: Some("%".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(literal.events.len(), 1);
        let search = db
            .query(MessagesQuery {
                q: Some("hello".into()),
                kind: FilterKind::Chat,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(search.events[0].id, third.id);
        let stats = db.stats().await.unwrap();
        assert_eq!(stats.stored_messages, 2);
        assert_eq!(stats.messages_24h, 2);
        assert_eq!(stats.activity.iter().map(|a| a.count).sum::<i64>(), 2);
    }
    #[tokio::test]
    async fn expiration_does_not_reuse_sequences() {
        let db = memory();
        let first = db
            .insert(EventType::Chat, Some("old".into()), None)
            .await
            .unwrap();
        db.run(|conn| {
            conn.execute(
                "UPDATE events SET timestamp = '2000-01-01T00:00:00.000Z'",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(db.cleanup(3).await.unwrap(), 1);
        let second = db
            .insert(EventType::Chat, Some("new".into()), None)
            .await
            .unwrap();
        assert!(second.id.ends_with("000000000002"));
        assert!(
            db.query(MessagesQuery {
                after: Some(first.id),
                ..Default::default()
            })
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn concurrent_writers_do_not_duplicate_or_drop_records() {
        let db = memory();
        let events = futures_util::future::try_join_all((0..100).map(|i| {
            let db = db.clone();
            async move { db.insert(EventType::Chat, Some(i.to_string()), None).await }
        }))
        .await
        .unwrap();
        let ids: std::collections::HashSet<_> = events.iter().map(|event| &event.id).collect();
        assert_eq!(ids.len(), 100);
        let page = db
            .query(MessagesQuery {
                limit: Some(100),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(page.events.len(), 100);
        assert!(!page.has_more);
        assert_eq!(db.stats().await.unwrap().stored_messages, 100);
    }
}
