use crate::llm::metrics::VllmMetrics;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

pub type MetricsFn = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Option<VllmMetrics>> + Send>> + Send + Sync>;

/// Adaptive concurrency for workers. Too many parallel long contexts evict each other's prefix cache and slow the
/// user down, so the limit follows vLLM's KV-cache usage and queue depth.
pub struct Governor {
    limit: AtomicUsize,
    max: usize,
    active: AtomicUsize,
    notify: Notify,
    read_metrics: Option<MetricsFn>,
    pub history: Mutex<Vec<(usize, Option<f64>, Option<f64>)>>,
    timer: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Governor {
    pub fn new(max: usize, read_metrics: Option<MetricsFn>, start: usize) -> Arc<Self> {
        let max = max.max(1);
        Arc::new(Governor { limit: AtomicUsize::new(start.clamp(1, max)), max, active: AtomicUsize::new(0), notify: Notify::new(), read_metrics, history: Mutex::new(vec![]), timer: Mutex::new(None) })
    }

    pub fn limit(&self) -> usize {
        self.limit.load(Ordering::SeqCst)
    }

    /// Pure decision so it can be tested without timers.
    pub fn next(limit: usize, max: usize, kv: Option<f64>, waiting: Option<f64>) -> usize {
        if kv.is_none() && waiting.is_none() {
            return limit;
        }
        if kv.unwrap_or(0.0) > 0.85 || waiting.unwrap_or(0.0) > 0.0 {
            return limit.saturating_sub(1).max(1);
        }
        if kv.unwrap_or(1.0) < 0.5 && waiting.unwrap_or(0.0) == 0.0 {
            return (limit + 1).min(max);
        }
        limit
    }

    pub async fn tick(&self) {
        let m = match &self.read_metrics {
            Some(f) => f().await,
            None => None,
        };
        let (kv, waiting) = m.map(|m| (m.kv_usage, m.waiting)).unwrap_or((None, None));
        let n = Self::next(self.limit(), self.max, kv, waiting);
        self.limit.store(n, Ordering::SeqCst);
        self.history.lock().unwrap().push((n, kv, waiting));
        self.notify.notify_waiters();
    }

    pub fn start(self: &Arc<Self>, every: Duration) {
        if self.read_metrics.is_none() || self.timer.lock().unwrap().is_some() {
            return;
        }
        let me = self.clone();
        *self.timer.lock().unwrap() = Some(tokio::spawn(async move {
            loop {
                tokio::time::sleep(every).await;
                me.tick().await;
            }
        }));
    }

    pub fn stop(&self) {
        if let Some(h) = self.timer.lock().unwrap().take() {
            h.abort();
        }
    }

    pub async fn run<T, F: Future<Output = T>>(&self, fut: F) -> T {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let cur = self.active.load(Ordering::SeqCst);
            if cur < self.limit() && self.active.compare_exchange(cur, cur + 1, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                break;
            }
            // wake periodically too, so a missed notification can never stall a worker
            let _ = tokio::time::timeout(Duration::from_millis(200), notified).await;
        }
        let out = fut.await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        self.notify.notify_waiters();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decisions() {
        assert_eq!(Governor::next(3, 4, Some(0.9), None), 2);
        assert_eq!(Governor::next(3, 4, Some(0.3), Some(0.0)), 4);
        assert_eq!(Governor::next(3, 4, Some(0.3), Some(2.0)), 2);
        assert_eq!(Governor::next(1, 4, Some(0.95), None), 1);
        assert_eq!(Governor::next(2, 4, None, None), 2);
    }

    #[tokio::test]
    async fn bounds_concurrency_and_follows_pressure() {
        let f: MetricsFn = Arc::new(|| Box::pin(async { Some(VllmMetrics { kv_usage: Some(0.2), waiting: Some(0.0), ..Default::default() }) }));
        let g = Governor::new(3, Some(f), 1);
        g.tick().await;
        g.tick().await; // limit -> 3
        assert_eq!(g.limit(), 3);
        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut hs = vec![];
        for _ in 0..6 {
            let (g, r, p) = (g.clone(), running.clone(), peak.clone());
            hs.push(tokio::spawn(async move {
                g.run(async {
                    let n = r.fetch_add(1, Ordering::SeqCst) + 1;
                    p.fetch_max(n, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    r.fetch_sub(1, Ordering::SeqCst);
                })
                .await
            }));
        }
        for h in hs {
            h.await.unwrap();
        }
        let pk = peak.load(Ordering::SeqCst);
        assert!((2..=3).contains(&pk), "peak {pk}");
    }
}
