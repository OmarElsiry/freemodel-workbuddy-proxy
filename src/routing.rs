use crate::{
    error::{AcpError, ProxyError},
    models::SessionRecord,
    session_store::{SessionStore, canonical_project},
    sidecar::SidecarManager,
};
use axum::http::{HeaderMap, StatusCode};
use parking_lot::Mutex;
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, Weak},
};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

#[derive(Default, Clone)]
pub struct GatewayLocks {
    locks: Arc<Mutex<HashMap<String, Weak<AsyncMutex<()>>>>>,
}
impl GatewayLocks {
    pub async fn acquire(&self, url: &str) -> OwnedMutexGuard<()> {
        let lock = {
            let mut map = self.locks.lock();
            map.retain(|_, v| v.strong_count() > 0);
            map.get(url).and_then(Weak::upgrade).unwrap_or_else(|| {
                let v = Arc::new(AsyncMutex::new(()));
                map.insert(url.into(), Arc::downgrade(&v));
                v
            })
        };
        lock.lock_owned().await
    }
}

pub async fn resolve(
    headers: &HeaderMap,
    messages: &[Value],
    store: &SessionStore,
    sidecars: &SidecarManager,
    default_project: &str,
) -> Result<(SessionRecord, String), ProxyError> {
    let requested = headers
        .get("x-workbuddy-session")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .trim();
    let hint = headers
        .get("x-workbuddy-project")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .trim();
    let project = canonical_project(if hint.is_empty() {
        default_project
    } else {
        hint
    })?;
    let session = if requested.is_empty() {
        store.automatic(&project, messages).await?
    } else {
        let session = store.get(requested).await?.ok_or_else(|| {
            ProxyError::Acp(
                AcpError::new("Unknown proxy session", "configuration")
                    .status(StatusCode::NOT_FOUND),
            )
        })?;
        if session.project != project {
            return Err(ProxyError::Acp(
                AcpError::new(
                    "Proxy session belongs to a different project",
                    "configuration",
                )
                .status(StatusCode::CONFLICT),
            ));
        }
        session
    };
    let url = sidecars.ensure(&session).await.map_err(|e| {
        ProxyError::Acp(
            AcpError::new(e.to_string(), "configuration").status(StatusCode::SERVICE_UNAVAILABLE),
        )
    })?;
    Ok((session, url))
}
