use std::ops::Deref;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use dashmap::DashMap;
use futures::future::BoxFuture;
use koharu_app::bus::EventBus;
use koharu_app::{App, AppSharedState};
use koharu_core::{AppEvent, DownloadProgress, JobSummary};
use koharu_runtime::RuntimeManager;
use serde::Serialize;

/// Builds the `App` (preparing runtime packages first). Supplied by the
/// binary, which owns the config; kept so a failed startup can be retried.
pub type Bootstrapper = Arc<
    dyn Fn(Arc<BootstrapManager>) -> BoxFuture<'static, anyhow::Result<Arc<App>>> + Send + Sync,
>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BootstrapState {
    Starting,
    Ready,
    Failed,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapStatus {
    pub state: BootstrapState,
    /// Why the last attempt failed; set when `state` is `failed`.
    pub error: Option<String>,
}

pub struct BootstrapManager {
    app: OnceLock<Arc<App>>,
    runtime: Arc<RuntimeManager>,
    shared: AppSharedState,
    bootstrapper: OnceLock<Bootstrapper>,
    /// Error from the last failed attempt; `None` while starting or ready.
    failure: Mutex<Option<String>>,
}

impl BootstrapManager {
    pub fn new(runtime: Arc<RuntimeManager>) -> Arc<Self> {
        Arc::new(Self {
            app: OnceLock::new(),
            runtime,
            shared: AppSharedState::default(),
            bootstrapper: OnceLock::new(),
            failure: Mutex::new(None),
        })
    }

    /// Run `bootstrapper` in the background. A failure is reported through
    /// [`Self::status`] (instead of ending the process) and can be re-run
    /// with [`Self::retry`].
    pub fn start(self: &Arc<Self>, bootstrapper: Bootstrapper) {
        if self.bootstrapper.set(bootstrapper).is_err() {
            tracing::warn!("bootstrap already started");
            return;
        }
        self.spawn_attempt();
    }

    /// Re-run a failed bootstrap. Returns `false` unless the last attempt
    /// failed (so it's a no-op while starting or once ready).
    pub fn retry(self: &Arc<Self>) -> bool {
        if self.failure().take().is_none() {
            return false;
        }
        self.spawn_attempt();
        true
    }

    pub fn status(&self) -> BootstrapStatus {
        if self.is_ready() {
            return BootstrapStatus {
                state: BootstrapState::Ready,
                error: None,
            };
        }
        match self.failure().clone() {
            Some(error) => BootstrapStatus {
                state: BootstrapState::Failed,
                error: Some(error),
            },
            None => BootstrapStatus {
                state: BootstrapState::Starting,
                error: None,
            },
        }
    }

    fn spawn_attempt(self: &Arc<Self>) {
        let Some(bootstrapper) = self.bootstrapper.get().cloned() else {
            return;
        };
        let this = self.clone();
        tokio::spawn(async move {
            match bootstrapper(this.clone()).await {
                Ok(app) => {
                    if this.set_app(app).is_err() {
                        tracing::warn!("app already initialized");
                    }
                }
                Err(err) => {
                    tracing::error!("failed to start Koharu: {err:#}");
                    *this.failure() = Some(format!("{err:#}"));
                }
            }
        });
    }

    fn failure(&self) -> MutexGuard<'_, Option<String>> {
        self.failure.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn app(&self) -> Option<Arc<App>> {
        self.app.get().cloned()
    }

    pub fn is_ready(&self) -> bool {
        self.app.get().is_some()
    }

    pub fn set_app(&self, app: Arc<App>) -> Result<(), Arc<App>> {
        self.app.set(app)
    }

    pub fn runtime(&self) -> Arc<RuntimeManager> {
        self.runtime.clone()
    }

    pub fn shared_state(&self) -> AppSharedState {
        self.shared.clone()
    }

    pub fn jobs(&self) -> Arc<DashMap<String, JobSummary>> {
        self.shared.jobs.clone()
    }

    pub fn downloads(&self) -> Arc<DashMap<String, DownloadProgress>> {
        self.shared.downloads.clone()
    }

    pub fn bus(&self) -> Arc<EventBus> {
        self.shared.bus.clone()
    }

    pub fn spawn_download_forwarder(&self) {
        let mut rx = self.runtime.subscribe_downloads();
        let downloads = self.downloads();
        let bus = self.bus();
        tokio::spawn(async move {
            while let Ok(progress) = rx.recv().await {
                downloads.insert(progress.id.clone(), progress.clone());
                bus.publish(AppEvent::DownloadProgress(progress));
            }
        });
    }
}

impl Deref for BootstrapManager {
    type Target = App;

    fn deref(&self) -> &Self::Target {
        self.app
            .get()
            .expect("bootstrap routes must guard app access until ready")
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use koharu_runtime::{ComputePolicy, RuntimeHttpConfig};

    use super::*;

    fn manager(root: &std::path::Path) -> Arc<BootstrapManager> {
        BootstrapManager::new(Arc::new(
            RuntimeManager::new_with_http(
                root,
                ComputePolicy::CpuOnly,
                RuntimeHttpConfig::default(),
            )
            .unwrap(),
        ))
    }

    async fn wait_for_failure(state: &BootstrapManager) -> BootstrapStatus {
        for _ in 0..200 {
            let status = state.status();
            if status.state == BootstrapState::Failed {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("bootstrap never reported failure");
    }

    #[tokio::test]
    async fn failed_start_is_reported_and_can_be_retried() {
        let root = tempfile::tempdir().unwrap();
        let state = manager(root.path());
        let attempts = Arc::new(AtomicUsize::new(0));
        let counter = attempts.clone();
        state.start(Arc::new(
            move |_| -> BoxFuture<'static, anyhow::Result<Arc<App>>> {
                let attempt = counter.fetch_add(1, Ordering::SeqCst) + 1;
                Box::pin(async move { Err(anyhow::anyhow!("download failed (attempt {attempt})")) })
            },
        ));

        let status = wait_for_failure(&state).await;
        assert_eq!(status.error.as_deref(), Some("download failed (attempt 1)"));

        assert!(state.retry());
        let status = wait_for_failure(&state).await;
        assert_eq!(status.error.as_deref(), Some("download failed (attempt 2)"));
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert!(!state.is_ready());
    }

    #[tokio::test]
    async fn retry_is_refused_unless_the_last_attempt_failed() {
        let root = tempfile::tempdir().unwrap();
        let state = manager(root.path());
        assert_eq!(state.status().state, BootstrapState::Starting);
        assert!(!state.retry());
    }
}
