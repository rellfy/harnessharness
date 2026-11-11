use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use tokio::select;
use tokio::sync::Notify;

/// A cloneable signal that cancels a running prompt.
/// Cancelling is sticky: every clone observes it, and it cannot be undone.
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    shared: Arc<CancelState>,
}

#[derive(Debug, Default)]
struct CancelState {
    is_cancelled: AtomicBool,
    notify: Notify,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.shared.is_cancelled.store(true, Ordering::SeqCst);
        self.shared.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.shared.is_cancelled.load(Ordering::SeqCst)
    }

    /// Resolve once the token is cancelled.
    pub async fn cancelled(&self) {
        let notified = self.shared.notify.notified();
        if self.is_cancelled() {
            return;
        }
        notified.await;
    }

    /// Run `future` until it completes or the token is cancelled, dropping it on cancel.
    pub(crate) async fn guard<T>(&self, future: impl Future<Output = T>) -> Option<T> {
        select! {
            biased;
            _ = self.cancelled() => None,
            output = future => Some(output),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::pending;
    use std::time::Duration;
    use tokio::time::sleep;

    #[tokio::test]
    async fn resolves_for_every_clone_after_cancel() {
        let token = CancelToken::new();
        let clone = token.clone();
        let waiter = tokio::spawn(async move { clone.cancelled().await });
        sleep(Duration::from_millis(10)).await;
        token.cancel();
        waiter.await.unwrap();
        assert!(token.is_cancelled());
        token.cancelled().await;
    }

    #[tokio::test]
    async fn drops_the_guarded_future_on_cancel() {
        let token = CancelToken::new();
        let canceller = token.clone();
        tokio::spawn(async move {
            sleep(Duration::from_millis(10)).await;
            canceller.cancel();
        });
        let output: Option<()> = token.guard(pending()).await;
        assert_eq!(output, None);
    }

    #[tokio::test]
    async fn returns_the_output_when_not_cancelled() {
        let token = CancelToken::new();
        assert_eq!(token.guard(async { 7 }).await, Some(7));
    }
}
