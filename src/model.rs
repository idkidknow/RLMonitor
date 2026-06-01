use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    Chat,
    Connected,
    Disconnected,
}

impl EventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Connected => "connected",
            Self::Disconnected => "disconnected",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatEvent {
    pub id: String,
    pub timestamp: String,
    pub event_type: EventType,
    pub content: Option<String>,
    pub raw_json: Option<String>,
}

#[derive(Deserialize)]
pub struct McMessage {
    pub json: String,
    #[serde(rename = "translatedText")]
    pub translated_text: Option<String>,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct MessagesQuery {
    pub before: Option<String>,
    pub after: Option<String>,
    pub limit: Option<u32>,
    pub q: Option<String>,
    #[serde(default)]
    pub kind: FilterKind,
}

#[derive(Debug, Default, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterKind {
    #[default]
    All,
    Chat,
    Connections,
}

#[derive(Serialize)]
pub struct MessagesPage {
    pub events: Vec<ChatEvent>,
    pub has_more: bool,
}

#[derive(Serialize)]
pub struct Activity {
    pub hour: String,
    pub count: i64,
}

#[derive(Serialize)]
pub struct Stats {
    pub stored_messages: i64,
    pub messages_24h: i64,
    pub last_message_at: Option<String>,
    pub activity: Vec<Activity>,
}
