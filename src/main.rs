mod config;
mod db;
mod http;
mod ingest;
mod model;
mod players;

use anyhow::{Context, Result};
use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};
use tokio::sync::{broadcast, watch};
use tracing::{error, info};

#[tokio::main]
async fn main() -> Result<()> {
    let config = Arc::new(config::Config::load()?);
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_new(&config.log_filter)
                .unwrap_or_else(|_| "info".into()),
        )
        .init();
    let db = db::Database::open(&config.database_path)?;
    let (events, _) = broadcast::channel(512);
    let state = http::AppState {
        db: db.clone(),
        events,
        connected: Arc::new(AtomicBool::new(false)),
        config: config.clone(),
        players: Arc::new(players::PlayerService::new(config.http_url.clone())?),
    };
    let listener = tokio::net::TcpListener::bind(config.listen_addr)
        .await
        .context("Cannot bind HTTP listener")?;
    let (shutdown, stop) = watch::channel(false);
    let ingestion = tokio::spawn(ingest::run(state.clone(), stop.clone()));
    let players = tokio::spawn(
        state
            .players
            .clone()
            .run(state.events.clone(), stop.clone()),
    );
    let mut cleanup_stop = stop.clone();
    let retention = config.retention_days;
    let cleanup = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(600));
        loop {
            tokio::select! {
                _ = cleanup_stop.changed() => break,
                _ = interval.tick() => match db.cleanup(retention).await {
                    Ok(count) if count > 0 => info!(count, "Expired events removed"),
                    Err(err) => error!(%err, "Retention cleanup failed"),
                    _ => {}
                }
            }
        }
    });
    info!(address = %listener.local_addr()?, "HTTP server ready");
    let shutdown_signal = shutdown.clone();
    let result = axum::serve(listener, http::router(state, stop))
        .with_graceful_shutdown(async move {
            signal().await;
            let _ = shutdown_signal.send(true);
        })
        .await;
    let _ = shutdown.send(true);
    ingestion.await.context("Ingestion task panicked")?;
    cleanup.await.context("Cleanup task panicked")?;
    players.await.context("Player polling task panicked")?;
    result.context("HTTP server failed")
}

async fn signal() {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} }
            }
            Err(err) => {
                error!(%err, "Cannot install SIGTERM handler");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}
