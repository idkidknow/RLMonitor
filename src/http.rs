use crate::{
    config::Config,
    db::{CursorExpired, Database},
    model::{ChatEvent, MessagesPage, MessagesQuery},
    players::{PlayerService, PlayersSnapshot, RankingsSnapshot, Update},
};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use std::{
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{broadcast, watch};
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use tracing::{error, warn};

#[derive(Clone)]
pub struct AppState {
    pub db: Database,
    pub events: broadcast::Sender<ChatEvent>,
    pub connected: Arc<AtomicBool>,
    pub config: Arc<Config>,
    pub players: Arc<PlayerService>,
}

#[derive(Clone)]
struct HttpState {
    app: AppState,
    stop: watch::Receiver<bool>,
}

pub fn router(app: AppState, stop: watch::Receiver<bool>) -> Router {
    let frontend = ServeDir::new(&app.config.frontend_dir)
        .not_found_service(ServeFile::new(app.config.frontend_dir.join("index.html")));
    Router::new()
        .route("/api/messages", get(messages))
        .route("/api/history", get(history))
        .route("/api/overview", get(overview))
        .route("/api/events", get(events))
        .route("/api/players", get(players))
        .route("/api/players/refresh", post(refresh_players))
        .route("/api/rankings", get(rankings))
        .route("/api/rankings/refresh", post(refresh_rankings))
        .route("/health", get(|| async { "OK" }))
        .nest(
            "/api",
            Router::new().fallback(|| async { StatusCode::NOT_FOUND }),
        )
        .fallback_service(frontend)
        .layer(TraceLayer::new_for_http())
        .with_state(HttpState { app, stop })
}

struct ApiError(StatusCode, &'static str);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({"error": self.1}))).into_response()
    }
}
impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        if err.downcast_ref::<CursorExpired>().is_some() {
            return Self(StatusCode::GONE, "Cursor expired; reload history");
        }
        error!(%err, "API operation failed");
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Storage is temporarily unavailable",
        )
    }
}

fn validate(query: &MessagesQuery) -> Result<(), ApiError> {
    if query.before.is_some() && query.after.is_some() {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Use either before or after",
        ));
    }
    if !(1..=100).contains(&query.limit.unwrap_or(50)) {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "limit must be between 1 and 100",
        ));
    }
    if query.q.as_ref().is_some_and(|q| q.len() > 256)
        || query
            .before
            .as_ref()
            .or(query.after.as_ref())
            .is_some_and(|id| id.len() > 128)
    {
        return Err(ApiError(StatusCode::BAD_REQUEST, "Query is too long"));
    }
    Ok(())
}

async fn history(
    State(state): State<HttpState>,
    Query(query): Query<MessagesQuery>,
) -> Result<Json<MessagesPage>, ApiError> {
    validate(&query)?;
    Ok(Json(state.app.db.query(query).await?))
}
// Preserve the original array response for existing API consumers.
async fn messages(
    State(state): State<HttpState>,
    Query(query): Query<MessagesQuery>,
) -> Result<Json<Vec<ChatEvent>>, ApiError> {
    validate(&query)?;
    Ok(Json(state.app.db.query(query).await?.events))
}

async fn overview(State(state): State<HttpState>) -> Result<Json<serde_json::Value>, ApiError> {
    let stats = state.app.db.stats().await?;
    Ok(Json(serde_json::json!({
        "connected": state.app.connected.load(Ordering::Acquire),
        "server_name": state.app.config.server_name,
        "retention_days": state.app.config.retention_days,
        "realitylink_address": state.app.config.realitylink_address,
        "stats": stats,
    })))
}

async fn players(State(state): State<HttpState>) -> Json<PlayersSnapshot> {
    Json(state.app.players.players().await)
}
async fn rankings(State(state): State<HttpState>) -> Json<RankingsSnapshot> {
    Json(state.app.players.rankings().await)
}
async fn refresh_players(
    State(state): State<HttpState>,
) -> Result<Json<PlayersSnapshot>, ApiError> {
    let service = state.app.players;
    Ok(Json(
        tokio::spawn(async move {
            service
                .refresh_players(Some(tokio::time::Instant::now()))
                .await
        })
        .await
        .map_err(refresh_task_error)?,
    ))
}
async fn refresh_rankings(
    State(state): State<HttpState>,
) -> Result<Json<RankingsSnapshot>, ApiError> {
    let service = state.app.players;
    Ok(Json(
        tokio::spawn(async move {
            service
                .refresh_rankings(None, tokio::time::Instant::now(), true)
                .await
        })
        .await
        .map_err(refresh_task_error)?,
    ))
}

fn refresh_task_error(error: tokio::task::JoinError) -> ApiError {
    error!(%error, "Player refresh task failed");
    ApiError(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Player refresh is temporarily unavailable",
    )
}

async fn events(
    State(state): State<HttpState>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let mut receiver = state.app.events.subscribe();
    let mut player_updates = state.app.players.updates.subscribe();
    let mut stop = state.stop;
    let stream = async_stream::stream! {
        let status = serde_json::json!({"connected": state.app.connected.load(Ordering::Acquire)});
        yield Ok(Event::default().event("status").data(status.to_string()));
        if let Ok(json) = serde_json::to_string(&state.app.players.players().await) {
            yield Ok(Event::default().event("players").data(json));
        }
        if let Ok(json) = serde_json::to_string(&state.app.players.rankings().await) {
            yield Ok(Event::default().event("rankings").data(json));
        }
        loop {
            tokio::select! {
                _ = stop.changed() => break,
                update = player_updates.recv() => match update {
                    Ok(update) => {
                        let (name, json) = match update {
                            Update::Players(snapshot) => ("players", serde_json::to_string(&snapshot)),
                            Update::Rankings(snapshot) => ("rankings", serde_json::to_string(&snapshot)),
                        };
                        if let Ok(json) = json { yield Ok(Event::default().event(name).data(json)); }
                    },
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        yield Ok(Event::default().event("player-sync").data("{}"));
                    },
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                received = receiver.recv() => match received {
                    Ok(event) => match serde_json::to_string(&event) {
                        Ok(json) => yield Ok(Event::default().id(&event.id).data(json)),
                        Err(err) => error!(%err, "Cannot serialize SSE event"),
                    },
                    Err(broadcast::error::RecvError::Lagged(count)) => {
                        warn!(count, "SSE client needs history recovery");
                        yield Ok(Event::default().event("sync").data("{}"));
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

#[cfg(test)]
pub fn test_state() -> AppState {
    let config = Config {
        realitylink_address: "127.0.0.1:39244".into(),
        http_url: "http://127.0.0.1:39244/".parse().unwrap(),
        ws_url: "ws://127.0.0.1:39244/minecraft-chat".into(),
        listen_addr: "127.0.0.1:0".parse().unwrap(),
        database_path: ":memory:".into(),
        frontend_dir: "/tmp/mc-chat-missing-dist".into(),
        server_name: "Test world".into(),
        retention_days: 3,
        log_filter: "info".into(),
    };
    let players = Arc::new(PlayerService::new(config.http_url.clone()).unwrap());
    AppState {
        db: Database::open(&config.database_path).unwrap(),
        config: Arc::new(config),
        events: broadcast::channel(512).0,
        connected: Arc::new(AtomicBool::new(false)),
        players,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unbounded_and_ambiguous_queries() {
        for limit in [0, 101, u32::MAX] {
            assert!(
                validate(&MessagesQuery {
                    limit: Some(limit),
                    ..Default::default()
                })
                .is_err()
            );
        }
        assert!(
            validate(&MessagesQuery {
                before: Some("a".into()),
                after: Some("b".into()),
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            validate(&MessagesQuery {
                q: Some("a".repeat(257)),
                ..Default::default()
            })
            .is_err()
        );
    }

    #[tokio::test]
    async fn preserves_legacy_api_and_exposes_history_and_live_status() {
        use axum::{
            body::{Body, to_bytes},
            http::Request,
        };
        use tower::ServiceExt;
        let state = test_state();
        state
            .db
            .insert(crate::model::EventType::Connected, None, None)
            .await
            .unwrap();
        state
            .db
            .insert(
                crate::model::EventType::Chat,
                Some("<Alex> hello".into()),
                Some("{}".into()),
            )
            .await
            .unwrap();
        let (_shutdown, stop) = watch::channel(false);
        let app = router(state, stop);
        for (path, status) in [
            ("/health", 200),
            ("/api/messages", 200),
            ("/api/history", 200),
            ("/api/players", 200),
            ("/api/rankings", 200),
            ("/api/history?limit=0", 400),
            ("/api/history?limit=-1", 400),
            ("/api/history?before=missing", 410),
            ("/api/unknown", 404),
        ] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status().as_u16(), status, "{path}");
            if path == "/api/messages" || path == "/api/history" {
                let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
                let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                if path == "/api/messages" {
                    assert!(json.is_array());
                } else {
                    assert_eq!(json["events"].as_array().unwrap().len(), 2);
                }
            }
        }
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/overview")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let json: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(
            json["connected"], false,
            "History must not determine current connection state"
        );
    }

    #[tokio::test]
    async fn sse_signals_recovery_when_a_subscriber_lags() {
        use axum::{body::Body, http::Request};
        use futures_util::StreamExt;
        use tower::ServiceExt;
        let mut state = test_state();
        state.events = broadcast::channel(1).0;
        let sender = state.events.clone();
        let (shutdown, stop) = watch::channel(false);
        let response = router(state, stop)
            .oneshot(
                Request::builder()
                    .uri("/api/events")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let mut stream = response.into_body().into_data_stream();
        let status = stream.next().await.unwrap().unwrap();
        assert!(String::from_utf8_lossy(&status).contains("event: status"));
        for name in ["players", "rankings"] {
            let snapshot = stream.next().await.unwrap().unwrap();
            assert!(String::from_utf8_lossy(&snapshot).contains(&format!("event: {name}")));
        }
        for id in ["1", "2"] {
            sender
                .send(ChatEvent {
                    id: id.into(),
                    timestamp: "2026-10-02T00:00:00.000Z".into(),
                    event_type: crate::model::EventType::Connected,
                    content: None,
                    raw_json: None,
                })
                .unwrap();
        }
        let sync = stream.next().await.unwrap().unwrap();
        assert!(String::from_utf8_lossy(&sync).contains("event: sync"));
        shutdown.send(true).unwrap();
        // One retained event can win select before shutdown; either way the stream ends.
        while stream.next().await.is_some() {}
    }
}
