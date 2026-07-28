//! Process-shutdown tracking for detached Axum WebSocket upgrade tasks.

use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicUsize, Ordering},
};

use tokio::sync::Notify;

const RUNNING: u8 = 0;
const DRAINING: u8 = 1;
const FORCE_CLOSING: u8 = 2;

#[derive(Clone)]
pub(crate) struct WebSocketLifecycle {
    inner: Arc<WebSocketLifecycleInner>,
}

struct WebSocketLifecycleInner {
    phase: AtomicU8,
    active: AtomicUsize,
    phase_changed: Notify,
    drained: Notify,
}

impl WebSocketLifecycle {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(WebSocketLifecycleInner {
                phase: AtomicU8::new(RUNNING),
                active: AtomicUsize::new(0),
                phase_changed: Notify::new(),
                drained: Notify::new(),
            }),
        }
    }

    pub(super) fn reserve(&self) -> Option<WebSocketSessionGuard> {
        if self.inner.phase.load(Ordering::Acquire) != RUNNING {
            return None;
        }
        self.inner.active.fetch_add(1, Ordering::AcqRel);
        if self.inner.phase.load(Ordering::Acquire) == RUNNING {
            Some(WebSocketSessionGuard {
                lifecycle: self.clone(),
            })
        } else {
            self.release();
            None
        }
    }

    pub(super) fn begin_draining(&self) {
        if self
            .inner
            .phase
            .compare_exchange(RUNNING, DRAINING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.inner.phase_changed.notify_waiters();
        }
    }

    pub(super) fn force_close(&self) {
        if self.inner.phase.swap(FORCE_CLOSING, Ordering::AcqRel) != FORCE_CLOSING {
            self.inner.phase_changed.notify_waiters();
        }
    }

    pub(super) fn is_draining(&self) -> bool {
        self.inner.phase.load(Ordering::Acquire) != RUNNING
    }

    pub(super) fn is_force_closing(&self) -> bool {
        self.inner.phase.load(Ordering::Acquire) == FORCE_CLOSING
    }

    pub(super) fn active(&self) -> usize {
        self.inner.active.load(Ordering::Acquire)
    }

    pub(super) async fn shutdown_requested(&self) {
        loop {
            if self.is_draining() {
                return;
            }
            let notified = self.inner.phase_changed.notified();
            if self.is_draining() {
                return;
            }
            notified.await;
        }
    }

    pub(super) async fn force_requested(&self) {
        loop {
            if self.is_force_closing() {
                return;
            }
            let notified = self.inner.phase_changed.notified();
            if self.is_force_closing() {
                return;
            }
            notified.await;
        }
    }

    pub(super) async fn wait_drained(&self) {
        loop {
            if self.active() == 0 {
                return;
            }
            let notified = self.inner.drained.notified();
            if self.active() == 0 {
                return;
            }
            notified.await;
        }
    }

    fn release(&self) {
        if self.inner.active.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.inner.drained.notify_waiters();
        }
    }
}

pub(super) struct WebSocketSessionGuard {
    lifecycle: WebSocketLifecycle,
}

impl Drop for WebSocketSessionGuard {
    fn drop(&mut self) {
        self.lifecycle.release();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::time::timeout;

    use super::WebSocketLifecycle;

    #[tokio::test]
    async fn tracks_drain_and_force_close_without_lost_notifications() {
        let lifecycle = WebSocketLifecycle::new();
        let guard = lifecycle
            .reserve()
            .expect("running lifecycle accepts upgrade");
        assert_eq!(lifecycle.active(), 1);

        lifecycle.begin_draining();
        assert!(lifecycle.reserve().is_none());
        lifecycle.shutdown_requested().await;
        assert!(
            timeout(Duration::from_millis(10), lifecycle.force_requested())
                .await
                .is_err()
        );
        assert!(
            timeout(Duration::from_millis(10), lifecycle.wait_drained())
                .await
                .is_err()
        );

        lifecycle.force_close();
        lifecycle.force_requested().await;
        drop(guard);
        lifecycle.wait_drained().await;
        assert_eq!(lifecycle.active(), 0);
    }
}
