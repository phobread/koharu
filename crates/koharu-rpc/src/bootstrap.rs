use std::ops::Deref;
use std::sync::{Arc, OnceLock};

use dashmap::DashMap;
use koharu_app::bus::EventBus;
use koharu_app::{App, AppSharedState};
use koharu_core::{AppEvent, DownloadProgress, JobSummary};
use koharu_runtime::RuntimeManager;
use tokio::sync::broadcast;

pub struct BootstrapManager {
    app: OnceLock<Arc<App>>,
    runtime: Arc<RuntimeManager>,
    shared: AppSharedState,
    /// Serializes config read-modify-write-persist-store so concurrent PATCH/secret writes cannot clobber fields (ArcSwap store is otherwise last-writer-wins).
    pub(crate) config_write_lock: tokio::sync::Mutex<()>,
}

impl BootstrapManager {
    pub fn new(runtime: Arc<RuntimeManager>) -> Arc<Self> {
        Arc::new(Self {
            app: OnceLock::new(),
            runtime,
            shared: AppSharedState::default(),
            config_write_lock: tokio::sync::Mutex::new(()),
        })
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
            loop {
                match rx.recv().await {
                    Ok(progress) => {
                        downloads.insert(progress.id.clone(), progress.clone());
                        bus.publish(AppEvent::DownloadProgress(progress));
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("download forwarder skipped {n} download events");
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
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
