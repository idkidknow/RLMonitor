use crate::model::{ChatEvent, EventType};
use anyhow::{Context, Result, ensure};
use chrono::Utc;
use futures_util::{StreamExt, stream};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::{Mutex, RwLock, broadcast, watch},
    time::Instant,
};
use tracing::warn;

pub const PLAYER_INTERVAL: Duration = Duration::from_secs(120);
pub const RANKING_INTERVAL: Duration = Duration::from_secs(600);
const EVENT_DELAY: Duration = Duration::from_secs(5);
const EVENT_COOLDOWN: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Player {
    pub uuid: String,
    pub name: String,
    #[serde(default)]
    pub online: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct PlayerStatistics {
    #[serde(flatten)]
    pub player: Player,
    pub play_time_ticks: Option<u64>,
    pub deaths: Option<u64>,
    pub updated_at: Option<String>,
    pub error: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Snapshot<T> {
    pub players: Vec<T>,
    pub updated_at: Option<String>,
    pub error: Option<String>,
    pub refreshing: bool,
    pub revision: u64,
}
impl<T> Default for Snapshot<T> {
    fn default() -> Self {
        Self {
            players: vec![],
            updated_at: None,
            error: None,
            refreshing: false,
            revision: 0,
        }
    }
}
pub type PlayersSnapshot = Snapshot<Player>;
pub type RankingsSnapshot = Snapshot<PlayerStatistics>;

#[derive(Clone)]
pub enum Update {
    Players(PlayersSnapshot),
    Rankings(RankingsSnapshot),
}

struct Resource<T> {
    snapshot: RwLock<Snapshot<T>>,
    // Also coalesces requests which arrive while an upstream refresh is in flight.
    gate: Mutex<Option<(Instant, bool)>>,
}
impl<T> Default for Resource<T> {
    fn default() -> Self {
        Self {
            snapshot: RwLock::new(Snapshot::default()),
            gate: Mutex::new(None),
        }
    }
}

pub struct PlayerService {
    client: reqwest::Client,
    base: url::Url,
    directory: Resource<Player>,
    rankings: Resource<PlayerStatistics>,
    pub updates: broadcast::Sender<Update>,
}

impl PlayerService {
    pub fn new(base: url::Url) -> Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .connect_timeout(Duration::from_secs(3))
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            base,
            directory: Resource::default(),
            rankings: Resource::default(),
            updates: broadcast::channel(32).0,
        })
    }

    pub async fn players(&self) -> PlayersSnapshot {
        self.directory.snapshot.read().await.clone()
    }
    pub async fn rankings(&self) -> RankingsSnapshot {
        let directory = self.players().await;
        let mut result = self.rankings.snapshot.read().await.clone();
        for row in &mut result.players {
            if let Some(current) = directory.players.iter().find(|p| p.uuid == row.player.uuid) {
                row.player = current.clone();
            }
        }
        result
    }

    async fn get<T: DeserializeOwned>(&self, segments: &[&str]) -> Result<T> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("Invalid RealityLink base URL"))?
            .pop_if_empty()
            .extend(segments);
        let mut response = self.client.get(url).send().await?.error_for_status()?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= 1024 * 1024,
                "RealityLink response exceeds 1 MiB"
            );
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).context("Invalid RealityLink response")
    }

    pub async fn refresh_players(&self, since: Option<Instant>) -> PlayersSnapshot {
        let mut last = self.directory.gate.lock().await;
        if last.is_some_and(|(time, _)| {
            since.map_or(time.elapsed() < PLAYER_INTERVAL, |since| time >= since)
        }) {
            return self.players().await;
        }
        {
            let mut snapshot = self.directory.snapshot.write().await;
            snapshot.refreshing = true;
            snapshot.revision += 1;
        }
        let _ = self.updates.send(Update::Players(self.players().await));
        let result = tokio::try_join!(
            self.get::<Vec<Player>>(&["online-players"]),
            self.get::<Vec<Player>>(&["cached-players"])
        );
        let mut snapshot = self.directory.snapshot.write().await;
        match result.and_then(|(online, cached)| merge_players(online, cached)) {
            Ok(players) => {
                snapshot.players = players;
                snapshot.updated_at = Some(Utc::now().to_rfc3339());
                snapshot.error = None;
            }
            Err(error) => {
                warn!(%error, "Cannot refresh RealityLink players");
                snapshot.error = Some(if snapshot.updated_at.is_some() {
                    "玩家列表更新失败，显示上次数据，请检查 RealityLink 连接".into()
                } else {
                    "无法读取玩家列表，请检查 RealityLink 连接后重试".into()
                });
            }
        }
        snapshot.refreshing = false;
        snapshot.revision += 1;
        *last = Some((Instant::now(), true));
        let result = snapshot.clone();
        drop(snapshot);
        let _ = self.updates.send(Update::Players(result.clone()));
        result
    }

    async fn stat(&self, uuid: &str, names: &[&str]) -> Result<Option<u64>> {
        for name in names {
            if let Some(value) = self.get::<Option<u64>>(&["stats", uuid, name]).await? {
                return Ok(Some(value));
            }
        }
        Ok(None)
    }

    pub async fn refresh_rankings(
        &self,
        targets: Option<&HashSet<String>>,
        since: Instant,
        membership: bool,
    ) -> RankingsSnapshot {
        let mut last = self.rankings.gate.lock().await;
        if last.is_some_and(|(time, full)| time >= since && (full || targets.is_some())) {
            return self.rankings().await;
        }
        {
            let mut snapshot = self.rankings.snapshot.write().await;
            snapshot.refreshing = true;
            snapshot.revision += 1;
        }
        let _ = self.updates.send(Update::Rankings(self.rankings().await));
        let directory = self.refresh_players(membership.then_some(since)).await;
        if directory.updated_at.is_none() {
            let mut snapshot = self.rankings.snapshot.write().await;
            snapshot.refreshing = false;
            snapshot.revision += 1;
            snapshot.error = Some("玩家列表不可用，暂时无法读取排行榜".into());
            *last = Some((Instant::now(), targets.is_none()));
            let result = snapshot.clone();
            drop(snapshot);
            let _ = self.updates.send(Update::Rankings(result.clone()));
            return result;
        }
        let old = self.rankings.snapshot.read().await.clone();
        let mut rows: BTreeMap<String, PlayerStatistics> = old
            .players
            .into_iter()
            .map(|row| (row.player.uuid.clone(), row))
            .collect();
        let mut selected: Vec<_> = directory
            .players
            .iter()
            .filter(|player| {
                !rows.contains_key(&player.uuid)
                    || targets.is_none_or(|names| {
                        names.contains(&player.uuid) || names.contains(&player.name)
                    })
            })
            .cloned()
            .collect();
        selected.sort_by_key(|player| {
            rows.get(&player.uuid)
                .and_then(|row| row.updated_at.clone())
        });
        // Bound upstream fan-out regardless of how many browsers are connected.
        let jobs = stream::iter(selected.iter().cloned().map(|player| async move {
            let (time, deaths) = tokio::join!(
                self.stat(
                    &player.uuid,
                    &[
                        "minecraft.custom:minecraft.play_time",
                        "minecraft.custom:minecraft.play_one_minute",
                        "stat.playOneMinute"
                    ]
                ),
                self.stat(
                    &player.uuid,
                    &["minecraft.custom:minecraft.deaths", "stat.deaths"]
                ),
            );
            (player, time, deaths)
        }))
        .buffer_unordered(4);
        tokio::pin!(jobs);
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut completed = HashSet::new();
        let mut failed = false;
        let mut succeeded = 0;
        while let Ok(Some((player, time, deaths))) =
            tokio::time::timeout_at(deadline, jobs.next()).await
        {
            completed.insert(player.uuid.clone());
            let previous = rows.remove(&player.uuid);
            let error = time.is_err() || deaths.is_err();
            failed |= error;
            if !error {
                succeeded += 1;
            }
            rows.insert(
                player.uuid.clone(),
                PlayerStatistics {
                    player,
                    play_time_ticks: time
                        .unwrap_or_else(|_| previous.as_ref().and_then(|p| p.play_time_ticks)),
                    deaths: deaths.unwrap_or_else(|_| previous.as_ref().and_then(|p| p.deaths)),
                    updated_at: if error {
                        previous.and_then(|p| p.updated_at)
                    } else {
                        Some(Utc::now().to_rfc3339())
                    },
                    error,
                },
            );
        }
        for player in selected.iter().filter(|p| !completed.contains(&p.uuid)) {
            failed = true;
            rows.entry(player.uuid.clone())
                .or_insert_with(|| PlayerStatistics {
                    player: player.clone(),
                    play_time_ticks: None,
                    deaths: None,
                    updated_at: None,
                    error: true,
                })
                .error = true;
        }
        rows.retain(|uuid, _| directory.players.iter().any(|p| &p.uuid == uuid));
        let mut snapshot = self.rankings.snapshot.write().await;
        snapshot.players = rows.into_values().collect();
        if succeeded > 0 || !failed {
            snapshot.updated_at = Some(Utc::now().to_rfc3339());
        }
        snapshot.error = if failed {
            Some("部分玩家统计更新失败，已保留上次数据，可手动重试".into())
        } else if directory.error.is_some() {
            Some("玩家名单更新失败，排行榜基于上次取得的名单，可手动重试".into())
        } else {
            None
        };
        snapshot.refreshing = false;
        snapshot.revision += 1;
        *last = Some((Instant::now(), targets.is_none()));
        drop(snapshot);
        let result = self.rankings().await;
        let _ = self.updates.send(Update::Rankings(result.clone()));
        result
    }

    pub async fn run(
        self: Arc<Self>,
        events: broadcast::Sender<ChatEvent>,
        stop: watch::Receiver<bool>,
    ) {
        tokio::join!(
            self.worker(false, events.subscribe(), stop.clone()),
            self.worker(true, events.subscribe(), stop)
        );
    }

    async fn worker(
        &self,
        statistics: bool,
        mut events: broadcast::Receiver<ChatEvent>,
        mut stop: watch::Receiver<bool>,
    ) {
        let period = if statistics {
            RANKING_INTERVAL
        } else {
            PLAYER_INTERVAL
        };
        let mut interval = tokio::time::interval_at(Instant::now() + period, period);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut due = Some(Instant::now());
        let mut observed = Instant::now();
        let mut full = true;
        let mut membership = false;
        let mut hinted = false;
        let mut names = HashSet::new();
        let mut last_event_refresh: Option<Instant> = None;
        loop {
            if *stop.borrow() {
                break;
            }
            let timer = due.unwrap_or_else(|| Instant::now() + period);
            tokio::select! {
                _ = stop.changed() => break,
                _ = interval.tick() => { full = true; observed = Instant::now(); due = Some(Instant::now()); },
                incoming = events.recv() => {
                    let hint = match incoming {
                        Ok(event) => refresh_hint(&event),
                        Err(broadcast::error::RecvError::Lagged(_)) => Some(Hint { membership: true, full: true, player: None }),
                        Err(broadcast::error::RecvError::Closed) => break,
                    };
                    if let Some(hint) = hint.filter(|hint| statistics || hint.membership) {
                        hinted = true;
                        if due.is_none() { observed = Instant::now(); }
                        full |= hint.full;
                        membership |= hint.membership;
                        if let Some(name) = hint.player { names.insert(name); }
                        let next = event_deadline(Instant::now(), last_event_refresh);
                        due = Some(due.map_or(next, |current| current.min(next)));
                    }
                },
                _ = tokio::time::sleep_until(timer), if due.is_some() => {
                    tokio::select! {
                        _ = stop.changed() => break,
                        _ = async {
                            if statistics { self.refresh_rankings((!full).then_some(&names), observed, membership).await; }
                            else { self.refresh_players(Some(observed)).await; }
                        } => {}
                    }
                    if hinted { last_event_refresh = Some(Instant::now()); }
                    due = None; full = false; membership = false; hinted = false; names.clear();
                }
            }
        }
    }
}

fn event_deadline(now: Instant, last: Option<Instant>) -> Instant {
    (now + EVENT_DELAY).max(last.map_or(now, |last| last + EVENT_COOLDOWN))
}

fn merge_players(online: Vec<Player>, cached: Vec<Player>) -> Result<Vec<Player>> {
    let mut players = BTreeMap::new();
    for (list, is_online) in [(cached, false), (online, true)] {
        for mut player in list {
            ensure!(
                valid_uuid(&player.uuid) && !player.name.trim().is_empty(),
                "Invalid RealityLink player"
            );
            player.uuid = player.uuid.to_ascii_lowercase();
            player.online = is_online;
            players.insert(player.uuid.clone(), player);
        }
    }
    let mut result: Vec<_> = players.into_values().collect();
    result.sort_by(|a, b| {
        b.online
            .cmp(&a.online)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.uuid.cmp(&b.uuid))
    });
    Ok(result)
}

fn valid_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

#[derive(Debug)]
struct Hint {
    membership: bool,
    full: bool,
    player: Option<String>,
}
fn refresh_hint(event: &ChatEvent) -> Option<Hint> {
    match event.event_type {
        EventType::Connected => Some(Hint {
            membership: true,
            full: true,
            player: None,
        }),
        EventType::Disconnected => None,
        EventType::Chat => {
            let raw: serde_json::Value = serde_json::from_str(event.raw_json.as_deref()?).ok()?;
            let key = raw["translate"].as_str()?;
            let membership = matches!(key, "multiplayer.player.joined" | "multiplayer.player.left");
            if !membership && !key.starts_with("death.") {
                return None;
            }
            let first = &raw["with"][0];
            let player = first["insertion"]
                .as_str()
                .or_else(|| first["hoverEvent"]["contents"]["id"].as_str())
                .or_else(|| first["text"].as_str())
                .filter(|name| !name.is_empty())
                .map(str::to_owned);
            Some(Hint {
                membership,
                full: player.is_none(),
                player,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::{Path, State},
        http::StatusCode,
        routing::get,
    };
    use serde_json::{Value, json};

    const A: &str = "00000000-0000-4000-8000-000000000001";
    const B: &str = "00000000-0000-4000-8000-000000000002";
    const C: &str = "00000000-0000-4000-8000-000000000003";

    #[derive(Default)]
    struct Upstream {
        replies: RwLock<BTreeMap<String, Value>>,
        fail: RwLock<HashSet<String>>,
        requests: Mutex<Vec<String>>,
    }
    struct Fixture {
        state: Arc<Upstream>,
        service: Arc<PlayerService>,
        server: tokio::task::JoinHandle<()>,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.server.abort();
        }
    }
    async fn reply(state: Arc<Upstream>, key: String) -> Result<Json<Value>, StatusCode> {
        state.requests.lock().await.push(key.clone());
        if state.fail.read().await.contains(&key) {
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
        Ok(Json(
            state
                .replies
                .read()
                .await
                .get(&key)
                .cloned()
                .unwrap_or(Value::Null),
        ))
    }
    async fn fixture() -> Fixture {
        let state = Arc::new(Upstream::default());
        state.replies.write().await.extend([
            ("online-players".into(), json!([{"name":"Alex", "uuid":A}])),
            (
                "cached-players".into(),
                json!([{"name":"OldAlex", "uuid":A},{"name":"Birch", "uuid":B}]),
            ),
            (
                format!("{A}/minecraft.custom:minecraft.play_time"),
                json!(72000),
            ),
            (format!("{A}/minecraft.custom:minecraft.deaths"), json!(0)),
            // Old Minecraft stat keys are tried only if the modern key is absent.
            (format!("{B}/stat.playOneMinute"), json!(144000)),
            (format!("{B}/stat.deaths"), json!(7)),
        ]);
        let app =
            Router::new()
                .route(
                    "/online-players",
                    get(|State(s): State<Arc<Upstream>>| async {
                        reply(s, "online-players".into()).await
                    }),
                )
                .route(
                    "/cached-players",
                    get(|State(s): State<Arc<Upstream>>| async {
                        reply(s, "cached-players".into()).await
                    }),
                )
                .route(
                    "/stats/{uuid}/{name}",
                    get(
                        |State(s): State<Arc<Upstream>>,
                         Path((uuid, name)): Path<(String, String)>| async move {
                            reply(s, format!("{uuid}/{name}")).await
                        },
                    ),
                )
                .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async {
            axum::serve(listener, app).await.unwrap();
        });
        Fixture {
            state,
            service: Arc::new(PlayerService::new(url::Url::parse(&base).unwrap()).unwrap()),
            server,
        }
    }
    fn player(uuid: &str, name: &str) -> Player {
        Player {
            uuid: uuid.into(),
            name: name.into(),
            online: false,
        }
    }
    fn event(key: &str, name: &str) -> ChatEvent {
        ChatEvent {
            id: "1".into(),
            timestamp: Utc::now().to_rfc3339(),
            event_type: EventType::Chat,
            content: Some(format!("{name} {key}")),
            raw_json: Some(
                json!({"translate":key,"with":[{"text":"","insertion":name}]}).to_string(),
            ),
        }
    }

    #[test]
    fn merges_by_uuid_and_validates_players() {
        let rows = merge_players(
            vec![player(A, "Alex"), player(C, "Cedar")],
            vec![player(A, "old"), player(B, "Birch")],
        )
        .unwrap();
        assert_eq!(
            rows.iter()
                .map(|p| (p.name.as_str(), p.online))
                .collect::<Vec<_>>(),
            [("Alex", true), ("Cedar", true), ("Birch", false)]
        );
        assert!(merge_players(vec![player("invalid", "Alex")], vec![]).is_err());
        assert!(merge_players(vec![], vec![player(A, " ")]).is_err());
    }
    #[test]
    fn hints_use_actual_translation_keys_and_throttle_bursts() {
        for key in ["multiplayer.player.joined", "multiplayer.player.left"] {
            let hint = refresh_hint(&event(key, "Alex")).unwrap();
            assert!(hint.membership && !hint.full);
            assert_eq!(hint.player.as_deref(), Some("Alex"));
        }
        assert!(
            !refresh_hint(&event("death.attack.mob", "Alex"))
                .unwrap()
                .membership
        );
        for key in ["chat.type.text", "chat.type.advancement.task", "unknown"] {
            assert!(refresh_hint(&event(key, "Alex")).is_none());
        }
        let mut connected = event("", "");
        connected.event_type = EventType::Connected;
        assert!(refresh_hint(&connected).unwrap().full);
        connected.event_type = EventType::Disconnected;
        assert!(refresh_hint(&connected).is_none());
        let now = Instant::now();
        assert_eq!(event_deadline(now, None), now + Duration::from_secs(5));
        assert_eq!(
            event_deadline(now + Duration::from_secs(1), Some(now)),
            now + Duration::from_secs(30)
        );
        assert_eq!(
            event_deadline(now + Duration::from_secs(40), Some(now)),
            now + Duration::from_secs(45)
        );
        assert_eq!(PLAYER_INTERVAL, Duration::from_secs(120));
        assert_eq!(RANKING_INTERVAL, Duration::from_secs(600));
    }
    #[tokio::test]
    async fn caches_snapshots_falls_back_on_null_and_preserves_failures() {
        let f = fixture().await;
        let snapshot = f
            .service
            .refresh_rankings(None, Instant::now(), false)
            .await;
        assert!(snapshot.error.is_none());
        let alex = snapshot
            .players
            .iter()
            .find(|p| p.player.uuid == A)
            .unwrap();
        assert_eq!((alex.play_time_ticks, alex.deaths), (Some(72000), Some(0)));
        let birch = snapshot
            .players
            .iter()
            .find(|p| p.player.uuid == B)
            .unwrap();
        assert_eq!(
            (birch.play_time_ticks, birch.deaths),
            (Some(144000), Some(7))
        );
        assert!(
            !f.state
                .requests
                .lock()
                .await
                .contains(&format!("{A}/stat.deaths")),
            "Zero must not trigger fallback"
        );
        let calls = f.state.requests.lock().await.len();
        for _ in 0..20 {
            f.service.players().await;
            f.service.rankings().await;
        }
        f.service.refresh_players(None).await;
        assert_eq!(
            f.state.requests.lock().await.len(),
            calls,
            "Cache reads never query RealityLink"
        );
        f.state.fail.write().await.extend([
            "online-players".into(),
            format!("{A}/minecraft.custom:minecraft.play_time"),
            format!("{B}/minecraft.custom:minecraft.play_time"),
        ]);
        let failed = f.service.refresh_players(Some(Instant::now())).await;
        assert!(failed.error.is_some() && !failed.refreshing);
        assert_eq!(failed.players.len(), 2);
        let failed = f
            .service
            .refresh_rankings(None, Instant::now(), false)
            .await;
        assert!(failed.error.is_some() && !failed.refreshing);
        assert_eq!(
            failed.updated_at, snapshot.updated_at,
            "All failed rows keep the last successful update time"
        );
        assert_eq!(failed.players[0].play_time_ticks, Some(72000));
        assert!(failed.players[0].error);
        f.state.fail.write().await.clear();
        f.state.replies.write().await.insert(
            format!("{A}/minecraft.custom:minecraft.play_time"),
            json!(90000),
        );
        let fresh = f
            .service
            .refresh_rankings(None, Instant::now(), false)
            .await;
        assert_eq!(fresh.players[0].play_time_ticks, Some(90000));
        assert!(
            fresh.error.as_deref().unwrap().contains("名单"),
            "A stale directory must still be reported after stats recover"
        );
        let fresh = f.service.refresh_rankings(None, Instant::now(), true).await;
        assert!(
            fresh.error.is_none(),
            "Manual full refresh also recovers the directory"
        );
    }
    #[tokio::test]
    async fn coalesces_concurrent_refreshes_and_retains_unknown_statistics() {
        let f = fixture().await;
        let since = Instant::now();
        tokio::join!(
            f.service.refresh_players(Some(since)),
            f.service.refresh_players(Some(since))
        );
        assert_eq!(f.state.requests.lock().await.len(), 2);
        f.state
            .replies
            .write()
            .await
            .retain(|key, _| !key.starts_with(B));
        let snapshot = f
            .service
            .refresh_rankings(None, Instant::now(), false)
            .await;
        let unknown = snapshot
            .players
            .iter()
            .find(|p| p.player.uuid == B)
            .unwrap();
        assert_eq!((unknown.play_time_ticks, unknown.deaths), (None, None));
        assert!(
            !unknown.error,
            "Missing vanilla stats are not transport errors"
        );
        let failed = fixture().await;
        failed
            .state
            .fail
            .write()
            .await
            .insert("online-players".into());
        let snapshot = failed
            .service
            .refresh_rankings(None, Instant::now(), false)
            .await;
        assert!(
            snapshot.error.is_some()
                && snapshot.players.is_empty()
                && snapshot.updated_at.is_none()
        );
    }
    #[tokio::test]
    async fn workers_refresh_on_death_and_join_without_refreshing_plain_chat() {
        let f = fixture().await;
        let (events, _) = broadcast::channel(32);
        let (shutdown, stop) = watch::channel(false);
        let service = f.service.clone();
        let task = tokio::spawn(service.run(events.clone(), stop));
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if f.service.rankings().await.updated_at.is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        f.state.requests.lock().await.clear();
        events.send(event("chat.type.text", "Alex")).unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(f.state.requests.lock().await.is_empty());
        // Reset the event cooldown after initial loading; initial loading is not an event refresh.
        f.state.replies.write().await.insert(
            "online-players".into(),
            json!([{"name":"Alex","uuid":A},{"name":"Birch","uuid":B}]),
        );
        events
            .send(event("multiplayer.player.joined", "Birch"))
            .unwrap();
        events.send(event("death.attack.fall", "Alex")).unwrap();
        tokio::time::timeout(Duration::from_secs(7), async {
            loop {
                if f.service.players().await.players.iter().all(|p| p.online)
                    && f.state
                        .requests
                        .lock()
                        .await
                        .iter()
                        .any(|key| key == &format!("{A}/minecraft.custom:minecraft.deaths"))
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let requests = f.state.requests.lock().await.clone();
        assert_eq!(
            requests
                .iter()
                .filter(|k| k.as_str() == "online-players")
                .count(),
            1
        );
        assert_eq!(
            requests
                .iter()
                .filter(|k| k.as_str() == "cached-players")
                .count(),
            1
        );
        shutdown.send(true).unwrap();
        task.await.unwrap();
    }
}
