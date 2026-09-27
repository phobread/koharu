//! One GPU engine at a time.
//!
//! Pipeline jobs and repair-brush strokes load and run engines
//! independently. On a 6 GB card two GPU engines don't fit, and eviction (see
//! `EXCLUSIVE_ENGINES` in `engine.rs`) only frees an engine once no other
//! run still holds it, so overlapping runs spill into system memory and
//! crawl. Each GPU engine load + run takes a turn here instead; turns are
//! first come, first served.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::{Semaphore, SemaphorePermit};

/// How often a job waiting for its turn re-checks its cancel flag.
const CANCEL_POLL: Duration = Duration::from_millis(200);

pub(crate) struct GpuGate(Semaphore);

impl Default for GpuGate {
    fn default() -> Self {
        Self(Semaphore::new(1))
    }
}

impl GpuGate {
    /// Wait for the GPU. `None` if `cancel` is set while waiting.
    pub async fn turn(&self, cancel: &AtomicBool) -> Option<SemaphorePermit<'_>> {
        let acquire = self.0.acquire();
        tokio::pin!(acquire);
        loop {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            tokio::select! {
                permit = &mut acquire => {
                    return Some(permit.expect("the GPU gate is never closed"));
                }
                _ = tokio::time::sleep(CANCEL_POLL) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[tokio::test]
    async fn a_second_run_waits_for_the_first_to_finish() {
        let gate = Arc::new(GpuGate::default());
        let never = AtomicBool::new(false);
        let first = gate.turn(&never).await.unwrap();

        let second = {
            let gate = gate.clone();
            tokio::spawn(async move { gate.turn(&AtomicBool::new(false)).await.is_some() })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!second.is_finished());

        drop(first);
        assert!(second.await.unwrap());
    }

    #[tokio::test]
    async fn a_cancelled_job_stops_waiting() {
        let gate = GpuGate::default();
        let never = AtomicBool::new(false);
        let _held = gate.turn(&never).await.unwrap();

        let cancel = AtomicBool::new(false);
        let (turn, ()) = tokio::join!(gate.turn(&cancel), async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel.store(true, Ordering::Relaxed);
        });
        assert!(turn.is_none());
    }
}
