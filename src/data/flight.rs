//! Work shared by the calls of this process that want the same thing at once (requirements R-NW-2
//! and R-WS-6; `doc/m2-plan.md`, C2b).
//!
//! The work runs in a task of its own, so that a call that is called off does not stop it while
//! another call waits for it: it stops only when every call that waits for it is called off. Other
//! processes take turns through the locks of the cache (requirement R-CA-4), which the work takes.

use std::{
    collections::HashMap,
    sync::{Arc, Weak},
};

use tokio::{sync::watch, task::AbortHandle};
use tracing::Instrument as _;

use super::fetches::Fetches;

/// The work in flight, by key.
#[derive(Debug)]
pub struct Flights<T> {
    flying: parking_lot::Mutex<HashMap<String, Weak<Flight<T>>>>,
}

impl<T> Default for Flights<T> {
    fn default() -> Self {
        Self {
            flying: parking_lot::Mutex::new(HashMap::new()),
        }
    }
}

/// One piece of work, held by the calls that wait for it. When the last of them lets it go, the
/// task stops, if it has not finished.
#[derive(Debug)]
struct Flight<T> {
    /// The result, once there is one.
    done: watch::Receiver<Option<T>>,
    task: AbortHandle,
}

impl<T> Drop for Flight<T> {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl<T: Clone + Send + Sync + 'static> Flights<T> {
    /// The result of the work for `key`: of the work in flight, or of the work that `start` makes,
    /// started now. `None` when the work ended without a result, as when it panicked.
    pub async fn join<F>(&self, key: &str, start: impl FnOnce() -> F) -> Option<T>
    where
        F: Future<Output = T> + Send + 'static,
    {
        let flight = self.flight(key, start);
        let mut done = flight.done.clone();
        let result = done
            .wait_for(Option::is_some)
            .await
            .ok()
            .and_then(|done| done.clone());
        drop(flight);
        result
    }

    /// The flight of `key`, started with `start` unless one is in flight.
    fn flight<F>(&self, key: &str, start: impl FnOnce() -> F) -> Arc<Flight<T>>
    where
        F: Future<Output = T> + Send + 'static,
    {
        let mut flying = self.flying.lock();
        if let Some(flight) = flying.get(key).and_then(Weak::upgrade) {
            return flight;
        }
        // Forget the flights that nobody waits for any more, so that the map does not grow.
        flying.retain(|_, flight| flight.strong_count() > 0);
        let (sender, done) = watch::channel(None);
        let work = start();
        // The requests that the work sends are noted for the call that started it (requirement
        // R-OBS-1).
        let fetches = Fetches::current().unwrap_or_default();
        let task = tokio::spawn(
            async move {
                let result = fetches.scope(work).await;
                // Every call may have stopped waiting meanwhile.
                drop(sender.send(Some(result)));
            }
            .in_current_span(),
        );
        let flight = Arc::new(Flight {
            done,
            task: task.abort_handle(),
        });
        flying.insert(key.to_owned(), Arc::downgrade(&flight));
        flight
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
        time::Duration,
    };

    use tokio::sync::Notify;

    use super::*;

    /// Work that counts its runs, waits for `go`, and returns `7`.
    fn work(runs: &Arc<AtomicUsize>, go: &Arc<Notify>) -> impl Future<Output = u32> + use<> {
        let (runs, go) = (Arc::clone(runs), Arc::clone(go));
        runs.fetch_add(1, Ordering::SeqCst);
        async move {
            go.notified().await;
            7
        }
    }

    #[tokio::test]
    async fn calls_at_the_same_time_share_one_run() {
        let flights = Arc::new(Flights::default());
        let (runs, go) = (Arc::new(AtomicUsize::new(0)), Arc::new(Notify::new()));
        let calls: Vec<_> = (0..3)
            .map(|_| {
                let (flights, runs, go) =
                    (Arc::clone(&flights), Arc::clone(&runs), Arc::clone(&go));
                tokio::spawn(async move { flights.join("key", || work(&runs, &go)).await })
            })
            .collect();
        tokio::time::sleep(Duration::from_millis(50)).await;
        go.notify_waiters();
        for call in calls {
            assert_eq!(call.await.expect("joined"), Some(7));
        }
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn the_work_goes_on_while_a_call_waits_for_it() {
        let flights = Arc::new(Flights::default());
        let (runs, go) = (Arc::new(AtomicUsize::new(0)), Arc::new(Notify::new()));
        let first = {
            let (flights, runs, go) = (Arc::clone(&flights), Arc::clone(&runs), Arc::clone(&go));
            tokio::spawn(async move { flights.join("key", || work(&runs, &go)).await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        let second = {
            let (flights, runs, go) = (Arc::clone(&flights), Arc::clone(&runs), Arc::clone(&go));
            tokio::spawn(async move { flights.join("key", || work(&runs, &go)).await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        // The call that started the work is called off.
        first.abort();
        tokio::time::sleep(Duration::from_millis(20)).await;
        go.notify_waiters();
        assert_eq!(second.await.expect("joined"), Some(7));
        assert_eq!(runs.load(Ordering::SeqCst), 1, "not started again");
    }

    #[tokio::test]
    async fn the_work_stops_when_every_call_is_called_off() {
        /// Says when it is dropped, as the work is when its task stops.
        struct Dropped(Arc<AtomicBool>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let flights = Arc::new(Flights::<u32>::default());
        let stopped = Arc::new(AtomicBool::new(false));
        let calls: Vec<_> = (0..2)
            .map(|_| {
                let (flights, stopped) = (Arc::clone(&flights), Arc::clone(&stopped));
                tokio::spawn(async move {
                    flights
                        .join("key", async move || {
                            let _dropped = Dropped(stopped);
                            std::future::pending::<u32>().await
                        })
                        .await
                })
            })
            .collect();
        tokio::time::sleep(Duration::from_millis(20)).await;
        for call in &calls {
            call.abort();
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            while !stopped.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the work stopped");
    }

    #[tokio::test]
    async fn work_that_panics_gives_no_result_and_is_started_again() {
        let flights = Flights::<u32>::default();
        let panicked = flights
            .join("key", async || panic!("the work panicked on purpose"))
            .await;
        assert_eq!(panicked, None);
        assert_eq!(flights.join("key", async || 7).await, Some(7));
    }
}
