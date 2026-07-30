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

pub async fn resolve_session(
    headers: &HeaderMap,
    messages: &[Value],
    store: &SessionStore,
    default_project: &str,
) -> Result<SessionRecord, ProxyError> {
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
    Ok(session)
}

pub async fn resolve(
    headers: &HeaderMap,
    messages: &[Value],
    store: &SessionStore,
    sidecars: &SidecarManager,
    default_project: &str,
) -> Result<(SessionRecord, String), ProxyError> {
    let session = resolve_session(headers, messages, store, default_project).await?;
    let url = sidecars.ensure(&session).await.map_err(|e| {
        ProxyError::Acp(
            AcpError::new(e.to_string(), "configuration").status(StatusCode::SERVICE_UNAVAILABLE),
        )
    })?;
    Ok((session, url))
}

#[cfg(test)]
mod tests {
    use super::GatewayLocks;
    use std::time::Duration;

    #[tokio::test]
    async fn same_gateway_serializes_and_releases_after_drop() {
        let locks = GatewayLocks::default();
        let first = locks.acquire("http://gateway").await;
        let second_locks = locks.clone();
        let mut second = tokio::spawn(async move { second_locks.acquire("http://gateway").await });
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut second)
                .await
                .is_err()
        );
        second.abort();
        drop(first);
        let guard = tokio::time::timeout(Duration::from_secs(1), locks.acquire("http://gateway"))
            .await
            .expect("lock is released after the owner drops");
        drop(guard);
    }

    #[tokio::test]
    async fn different_gateways_do_not_block_each_other() {
        let locks = GatewayLocks::default();
        let _first = locks.acquire("http://gateway-a").await;
        let second = tokio::time::timeout(
            Duration::from_millis(100),
            locks.acquire("http://gateway-b"),
        )
        .await;
        assert!(second.is_ok());
    }
}
