use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ModelInfo {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub owned_by: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SessionRecord {
    pub id: String,
    pub title: String,
    pub project: String,
    pub automatic: bool,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub history: Vec<Value>,
    #[serde(default)]
    pub sidecar: Map<String, Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionFile {
    pub version: u32,
    pub sessions: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NormalizedEvent {
    TextDelta(String),
    Completed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ErrorEnvelope {
    pub error: ErrorBody,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ErrorBody {
    pub message: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub code: Value,
}
