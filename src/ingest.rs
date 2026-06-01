use crate::{
    http::AppState,
    model::{EventType, McMessage},
};
use futures_util::{SinkExt, StreamExt};
use std::{sync::atomic::Ordering, time::Duration};
use tokio::sync::watch;
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};
use tracing::{error, info, warn};

pub async fn run(state: AppState, mut stop: watch::Receiver<bool>) {
    let mut retry_seconds = 1;
    loop {
        if *stop.borrow() {
            break;
        }
        let ws_config = WebSocketConfig::default()
            .max_message_size(Some(1024 * 1024))
            .max_frame_size(Some(1024 * 1024));
        let connection = tokio::select! {
            _ = stop.changed() => break,
            result = tokio::time::timeout(Duration::from_secs(15), connect_async_with_config(&state.config.ws_url, Some(ws_config), true)) => result,
        };
        match connection {
            Ok(Ok((mut socket, _))) => {
                info!("RealityLink connected");
                retry_seconds = 1;
                state.connected.store(true, Ordering::Release);
                publish(&state, EventType::Connected, None, None).await;
                let mut deadline = tokio::time::Instant::now() + Duration::from_secs(75);
                let mut heartbeat = tokio::time::interval_at(
                    tokio::time::Instant::now() + Duration::from_secs(20),
                    Duration::from_secs(20),
                );
                heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    let incoming = tokio::select! {
                        _ = stop.changed() => { let _ = tokio::time::timeout(Duration::from_secs(2), socket.close(None)).await; break; },
                        _ = heartbeat.tick() => {
                            if !matches!(tokio::time::timeout(Duration::from_secs(5), socket.send(Message::Ping(Vec::new().into()))).await, Ok(Ok(()))) {
                                warn!("RealityLink ping could not be sent"); break;
                            }
                            continue;
                        },
                        incoming = tokio::time::timeout_at(deadline, socket.next()) => incoming,
                    };
                    if incoming.is_ok() {
                        deadline = tokio::time::Instant::now() + Duration::from_secs(75);
                    }
                    match incoming {
                        Ok(Some(Ok(Message::Text(text)))) => {
                            match serde_json::from_str::<McMessage>(&text) {
                                Ok(message) => {
                                    let content = message
                                        .translated_text
                                        .unwrap_or_else(|| message.json.clone());
                                    publish(
                                        &state,
                                        EventType::Chat,
                                        Some(content),
                                        Some(message.json),
                                    )
                                    .await;
                                }
                                Err(err) => warn!(%err, "Invalid RealityLink message ignored"),
                            }
                        }
                        Ok(Some(Ok(Message::Ping(_)))) => {
                            // tungstenite queues the protocol Pong on read; flush it promptly.
                            if !matches!(
                                tokio::time::timeout(Duration::from_secs(5), socket.flush()).await,
                                Ok(Ok(()))
                            ) {
                                warn!("Cannot send WebSocket pong");
                                break;
                            }
                        }
                        Ok(Some(Ok(Message::Close(_)))) | Ok(None) => break,
                        Ok(Some(Err(err))) => {
                            warn!(%err, "RealityLink read failed");
                            break;
                        }
                        Err(_) => {
                            warn!("RealityLink heartbeat timed out");
                            break;
                        }
                        _ => {}
                    }
                }
                state.connected.store(false, Ordering::Release);
                publish(&state, EventType::Disconnected, None, None).await;
            }
            Ok(Err(err)) => warn!(%err, "RealityLink connection failed"),
            Err(_) => warn!("RealityLink connection timed out"),
        }
        if *stop.borrow() {
            break;
        }
        tokio::select! {
            _ = stop.changed() => break,
            _ = tokio::time::sleep(Duration::from_secs(retry_seconds)) => {}
        }
        retry_seconds = (retry_seconds * 2).min(30);
    }
}

async fn publish(state: &AppState, kind: EventType, content: Option<String>, raw: Option<String>) {
    match state.db.insert(kind, content, raw).await {
        Ok(event) => {
            let _ = state.events.send(event);
        }
        Err(err) => error!(%err, "Event could not be persisted; not broadcasting uncommitted data"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_realitylink_payload_and_optional_translation() {
        let message: McMessage =
            serde_json::from_str(r#"{"json":"{\"text\":\"hello\"}","translatedText":"你好"}"#)
                .unwrap();
        assert_eq!(message.translated_text.as_deref(), Some("你好"));
        let fallback: McMessage =
            serde_json::from_str(r#"{"json":"{\"text\":\"hello\"}"}"#).unwrap();
        assert!(fallback.translated_text.is_none());
    }

    #[tokio::test]
    async fn websocket_ingests_messages_answers_ping_and_shuts_down() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut state = crate::http::test_state();
        std::sync::Arc::get_mut(&mut state.config).unwrap().ws_url =
            format!("ws://{}/minecraft-chat", listener.local_addr().unwrap());
        let mut events = state.events.subscribe();
        let (shutdown, stop) = watch::channel(false);
        let task = tokio::spawn(run(state.clone(), stop));
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        socket
            .send(Message::Text(
                r#"{"json":"{\"text\":\"hello\"}","translatedText":"<Alex> 你好"}"#.into(),
            ))
            .await
            .unwrap();
        socket.send(Message::Text("invalid".into())).await.unwrap();
        socket
            .send(Message::Ping(vec![1, 2, 3].into()))
            .await
            .unwrap();
        let pong = tokio::time::timeout(Duration::from_secs(3), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(pong, Message::Pong(vec![1, 2, 3].into()));
        let first = tokio::time::timeout(Duration::from_secs(3), events.recv())
            .await
            .unwrap()
            .unwrap();
        let chat = tokio::time::timeout(Duration::from_secs(3), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(first.event_type, EventType::Connected));
        assert_eq!(chat.content.as_deref(), Some("<Alex> 你好"));
        shutdown.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap();
        assert!(!state.connected.load(Ordering::Acquire));
        let last = events.recv().await.unwrap();
        assert!(matches!(last.event_type, EventType::Disconnected));
        assert_eq!(
            state
                .db
                .query(Default::default())
                .await
                .unwrap()
                .events
                .len(),
            3
        );
    }
}
