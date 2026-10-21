//! Work shared by the calls of this process that want the same thing at once (requirements R-NW-2
//! and R-WS-6; `doc/m2-plan.md`, C2b and C3).
//!
//! The work runs in a task of its own, so that a call that is called off does not stop it while
//! another call waits for it: it stops only when every call that waits for it is called off. A
//! call waits for it until a deadline (requirement R-ARCH-6); the work then goes on until it ends,
//! and is no longer stopped when calls are called off, so that a later call finds what it made.
//! Other processes take turns through the locks of the cache (requirement R-CA-4), which the work
//! takes.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        Weak,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
    },
};

use tokio::{sync::watch, task::AbortHandle, time::Instant};
use tracing::Instrument as _;

use super::fetches::Fetches;

/// The results that [`Flights::handing_over`] keeps for the next call, by key, with the number of
/// their flight.
type Left<T> = parking_lot::Mutex<HashMap<String, (u64, T)>>;

/// The work in flight, by key.
#[derive(Debug)]
pub struct Flights<T> {
    flying: parking_lot::Mutex<HashMap<String, Weak<Flight<T>>>>,
    /// With [`Self::handing_over`], the results of work that went on past a deadline, by key,
    /// with the number of their flight, for the next call (decided in M2's D2).
    left: Option<Arc<Left<T>>>,
    /// Numbers the flights.
    next: AtomicU64,
}

impl<T> Flights<T> {
    /// Flights that hand the result of work that went on past a deadline to the next call for its
    /// key, once: what is made anew for each call, as a local build until D4, would else be lost,
    /// and a call that comes after it would start it again (decided in M2's D2).
    #[must_use]
    pub fn handing_over() -> Self {
        Self {
            left: Some(Arc::default()),
            ..Self::default()
        }
    }
}

impl<T> Default for Flights<T> {
    fn default() -> Self {
        Self {
            flying: parking_lot::Mutex::new(HashMap::new()),
            left: None,
            next: AtomicU64::new(0),
        }
    }
}

/// What a piece of work is doing, which a call that stops waiting for it tells (requirement
/// R-OUT-9, `building_reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Doing {
    /// Fetching what it is made from.
    Fetching,
    /// Parsing and indexing it.
    Indexing,
    /// Waiting for a lock that another cargo holds, as that of the package cache.
    WaitingForLock,
    /// Building the documentation locally (decided in M2's D2).
    Building,
}

impl Doing {
    /// Its name in responses.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fetching => "fetching",
            Self::Indexing => "indexing",
            Self::WaitingForLock => "waiting_for_lock",
            Self::Building => "building",
        }
    }

    /// What it is, for people: `fetching serde 1.0.228`.
    #[must_use]
    pub const fn as_words(self) -> &'static str {
        match self {
            Self::Fetching => "fetching",
            Self::Indexing => "indexing",
            Self::WaitingForLock => "waiting for a cargo lock for",
            Self::Building => "building the documentation of",
        }
    }
}

/// What a piece of work is doing now, which the work sets.
#[derive(Debug)]
pub struct Stage(AtomicU8);

impl Default for Stage {
    /// Work starts by fetching.
    fn default() -> Self {
        Self(AtomicU8::new(Doing::Fetching as u8))
    }
}

impl Stage {
    /// The work now does `doing`.
    pub fn set(&self, doing: Doing) {
        self.0.store(doing as u8, Ordering::Relaxed);
    }

    /// What the work does now.
    #[must_use]
    pub fn get(&self) -> Doing {
        match self.0.load(Ordering::Relaxed) {
            doing if doing == Doing::Indexing as u8 => Doing::Indexing,
            doing if doing == Doing::WaitingForLock as u8 => Doing::WaitingForLock,
            doing if doing == Doing::Building as u8 => Doing::Building,
            _ => Doing::Fetching,
        }
    }
}

/// How waiting for a piece of work ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Joined<T> {
    /// It ended with this result.
    Done(T),
    /// It ended without a result, as when it panicked.
    Ended,
    /// It had not ended by the deadline, and goes on: it was doing this then.
    Going(Doing),
}

/// One piece of work, held by the calls that wait for it. When the last of them lets it go, the
/// task stops, if it has not finished, unless a call stopped waiting for it at its deadline.
#[derive(Debug)]
struct Flight<T> {
    /// The result, once there is one.
    done: watch::Receiver<Option<T>>,
    stage: Arc<Stage>,
    /// A call stopped waiting for it at its deadline, so that it goes on until it ends, and its
    /// result is handed to the next call with [`Flights::handing_over`].
    left_going: Arc<AtomicBool>,
    /// Its number among the flights.
    number: u64,
    task: AbortHandle,
}

impl<T: Send + Sync + 'static> Flight<T> {
    /// Lets the work go on until it ends, even when every call that waits for it is called off:
    /// a call has told that it goes on (requirement R-WS-6).
    fn leave_going(self: Arc<Self>) {
        if self.left_going.swap(true, Ordering::Relaxed) {
            return;
        }
        let mut done = self.done.clone();
        tokio::spawn(async move {
            // It ends with a result or without one.
            drop(done.wait_for(Option::is_some).await);
            drop(self);
        });
    }
}

impl<T> Drop for Flight<T> {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl<T: Clone + Send + Sync + 'static> Flights<T> {
    /// The result of the work for `key`: of the work in flight, or of the work that `start`
    /// makes, started now, which it gives the [`Stage`] to set. When the work has not ended by
    /// `until`, it goes on, and [`Joined::Going`] says what it is doing.
    ///
    /// Meanwhile, the call in scope notes that it waits for work on `what` (`serde 1.0.228`),
    /// for its progress notifications (requirement R-WS-6).
    pub async fn join<F>(
        &self,
        key: &str,
        what: &str,
        until: Instant,
        start: impl FnOnce(Arc<Stage>) -> F,
    ) -> Joined<T>
    where
        F: Future<Output = T> + Send + 'static,
    {
        self.joined(key, what, Some(until), start).await
    }

    /// [`Self::join`] without a deadline: the result, or `None` when the work ended without one.
    pub async fn wait<F>(
        &self,
        key: &str,
        what: &str,
        start: impl FnOnce(Arc<Stage>) -> F,
    ) -> Option<T>
    where
        F: Future<Output = T> + Send + 'static,
    {
        match self.joined(key, what, None, start).await {
            Joined::Done(result) => Some(result),
            Joined::Ended | Joined::Going(_) => None,
        }
    }

    /// [`Self::join`], with no deadline when `until` is `None`.
    async fn joined<F>(
        &self,
        key: &str,
        what: &str,
        until: Option<Instant>,
        start: impl FnOnce(Arc<Stage>) -> F,
    ) -> Joined<T>
    where
        F: Future<Output = T> + Send + 'static,
    {
        if let Some(result) = self.take_left(key) {
            return Joined::Done(result);
        }
        let flight = self.flight(key, start);
        let _waiting = super::waits::waiting(what, &flight.stage);
        let mut done = flight.done.clone();
        let ended = done.wait_for(Option::is_some);
        let waited = match until {
            Some(until) => tokio::time::timeout_at(until, ended).await,
            None => Ok(ended.await),
        }
        .map(|done| done.ok().and_then(|done| done.clone()));
        match waited {
            Ok(Some(result)) => {
                // This call has it: it is not handed over again.
                self.forget_left(key, flight.number);
                Joined::Done(result)
            },
            Ok(None) => Joined::Ended,
            Err(_) => {
                let doing = flight.stage.get();
                flight.leave_going();
                Joined::Going(doing)
            },
        }
    }

    /// The result for `key` that work which went on past a deadline left, once.
    fn take_left(&self, key: &str) -> Option<T> {
        self.left
            .as_ref()
            .and_then(|left| left.lock().remove(key))
            .map(|(_, result)| result)
    }

    /// Forgets the result for `key` that the flight `number` left, which a call has had.
    fn forget_left(&self, key: &str, number: u64) {
        if let Some(left) = &self.left {
            let mut left = left.lock();
            if left.get(key).is_some_and(|(of, _)| *of == number) {
                left.remove(key);
            }
        }
    }

    /// The flight of `key`, started with `start` unless one is in flight.
    fn flight<F>(&self, key: &str, start: impl FnOnce(Arc<Stage>) -> F) -> Arc<Flight<T>>
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
        let stage = Arc::new(Stage::default());
        let work = start(Arc::clone(&stage));
        let left_going = Arc::new(AtomicBool::new(false));
        let number = self.next.fetch_add(1, Ordering::Relaxed);
        let handing = self
            .left
            .as_ref()
            .map(|left| (Arc::clone(left), Arc::clone(&left_going), key.to_owned()));
        // The requests that the work sends are noted for the call that started it (requirement
        // R-OBS-1).
        let fetches = Fetches::current().unwrap_or_default();
        let task = tokio::spawn(
            async move {
                let result = fetches.scope(work).await;
                // Before it is sent, so that a call that has it forgets it.
                if let Some((left, left_going, key)) = handing
                    && left_going.load(Ordering::Relaxed)
                {
                    left.lock().insert(key, (number, result.clone()));
                }
                // Every call may have stopped waiting meanwhile.
                drop(sender.send(Some(result)));
            }
            .in_current_span(),
        );
        let flight = Arc::new(Flight {
            done,
            stage,
            left_going,
            number,
            task: task.abort_handle(),
        });
        flying.insert(key.to_owned(), Arc::downgrade(&flight));
        flight
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::atomic::AtomicUsize, time::Duration};

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

    /// A deadline that a test does not reach.
    fn later() -> Instant {
        Instant::now()
            .checked_add(Duration::from_secs(60))
            .expect("in range")
    }

    /// Says when it is dropped, as the work is when its task stops.
    struct Dropped(Arc<AtomicBool>);

    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    /// A deadline that a test passes at once.
    fn soon() -> Instant {
        Instant::now()
            .checked_add(Duration::from_millis(20))
            .expect("in range")
    }

    /// Joins the work of `key` in a task of its own, with a deadline that it does not reach.
    fn join_later(
        flights: &Arc<Flights<u32>>,
        runs: &Arc<AtomicUsize>,
        go: &Arc<Notify>,
    ) -> tokio::task::JoinHandle<Joined<u32>> {
        let (flights, runs, go) = (Arc::clone(flights), Arc::clone(runs), Arc::clone(go));
        tokio::spawn(async move {
            flights
                .join("key", "work", later(), |_| work(&runs, &go))
                .await
        })
    }

    #[tokio::test]
    async fn the_result_of_work_that_went_on_goes_to_the_next_call_once() {
        let flights = Arc::new(Flights::handing_over());
        let (runs, go) = (Arc::new(AtomicUsize::new(0)), Arc::new(Notify::new()));
        let first = flights
            .join("key", "work", soon(), |_| work(&runs, &go))
            .await;
        assert!(matches!(first, Joined::Going(_)), "{first:?}");
        go.notify_waiters();
        tokio::time::sleep(Duration::from_millis(50)).await;
        // The next call has it, without running the work again.
        let second = flights
            .join("key", "work", later(), |_| work(&runs, &go))
            .await;
        assert_eq!(second, Joined::Done(7));
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        // Once.
        let third = join_later(&flights, &runs, &go);
        tokio::time::sleep(Duration::from_millis(20)).await;
        go.notify_waiters();
        assert_eq!(third.await.expect("joined"), Joined::Done(7));
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_result_that_a_call_waited_for_is_not_handed_over_again() {
        let flights = Arc::new(Flights::handing_over());
        let (runs, go) = (Arc::new(AtomicUsize::new(0)), Arc::new(Notify::new()));
        let first = flights
            .join("key", "work", soon(), |_| work(&runs, &go))
            .await;
        assert!(matches!(first, Joined::Going(_)), "{first:?}");
        // A call that joins the work that goes on has its result.
        let second = join_later(&flights, &runs, &go);
        tokio::time::sleep(Duration::from_millis(20)).await;
        go.notify_waiters();
        assert_eq!(second.await.expect("joined"), Joined::Done(7));
        tokio::time::sleep(Duration::from_millis(20)).await;
        let third = join_later(&flights, &runs, &go);
        tokio::time::sleep(Duration::from_millis(20)).await;
        go.notify_waiters();
        assert_eq!(third.await.expect("joined"), Joined::Done(7));
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn without_handing_over_a_call_after_the_work_runs_it_again() {
        let flights = Arc::new(Flights::default());
        let (runs, go) = (Arc::new(AtomicUsize::new(0)), Arc::new(Notify::new()));
        let first = flights
            .join("key", "work", soon(), |_| work(&runs, &go))
            .await;
        assert!(matches!(first, Joined::Going(_)), "{first:?}");
        go.notify_waiters();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let second = join_later(&flights, &runs, &go);
        tokio::time::sleep(Duration::from_millis(20)).await;
        go.notify_waiters();
        assert_eq!(second.await.expect("joined"), Joined::Done(7));
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn calls_at_the_same_time_share_one_run() {
        let flights = Arc::new(Flights::default());
        let (runs, go) = (Arc::new(AtomicUsize::new(0)), Arc::new(Notify::new()));
        let calls: Vec<_> = (0..3)
            .map(|_| {
                let (flights, runs, go) =
                    (Arc::clone(&flights), Arc::clone(&runs), Arc::clone(&go));
                tokio::spawn(async move {
                    flights
                        .join("key", "work", later(), |_| work(&runs, &go))
                        .await
                })
            })
            .collect();
        tokio::time::sleep(Duration::from_millis(50)).await;
        go.notify_waiters();
        for call in calls {
            assert_eq!(call.await.expect("joined"), Joined::Done(7));
        }
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn the_work_goes_on_while_a_call_waits_for_it() {
        let flights = Arc::new(Flights::default());
        let (runs, go) = (Arc::new(AtomicUsize::new(0)), Arc::new(Notify::new()));
        let first = {
            let (flights, runs, go) = (Arc::clone(&flights), Arc::clone(&runs), Arc::clone(&go));
            tokio::spawn(async move {
                flights
                    .join("key", "work", later(), |_| work(&runs, &go))
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        let second = {
            let (flights, runs, go) = (Arc::clone(&flights), Arc::clone(&runs), Arc::clone(&go));
            tokio::spawn(async move {
                flights
                    .join("key", "work", later(), |_| work(&runs, &go))
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        // The call that started the work is called off.
        first.abort();
        tokio::time::sleep(Duration::from_millis(20)).await;
        go.notify_waiters();
        assert_eq!(second.await.expect("joined"), Joined::Done(7));
        assert_eq!(runs.load(Ordering::SeqCst), 1, "not started again");
    }

    #[tokio::test]
    async fn the_work_stops_when_every_call_is_called_off() {
        let flights = Arc::new(Flights::<u32>::default());
        let stopped = Arc::new(AtomicBool::new(false));
        let calls: Vec<_> = (0..2)
            .map(|_| {
                let (flights, stopped) = (Arc::clone(&flights), Arc::clone(&stopped));
                tokio::spawn(async move {
                    flights
                        .join("key", "work", later(), async move |_| {
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
            .join("key", "work", later(), async |_| {
                panic!("the work panicked on purpose")
            })
            .await;
        assert_eq!(panicked, Joined::Ended);
        assert_eq!(
            flights.join("key", "work", later(), async |_| 7).await,
            Joined::Done(7)
        );
    }

    /// A call that stops waiting at its deadline tells what the work is doing, and the work goes
    /// on even when every later call is called off, for the call after them (requirements
    /// R-ARCH-6 and R-WS-6).
    #[tokio::test]
    async fn work_past_the_deadline_goes_on_and_tells_what_it_does() {
        let flights = Arc::new(Flights::<u32>::default());
        let (indexing, go) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
        let stopped = Arc::new(AtomicBool::new(false));
        let start = {
            let (indexing, go, stopped) =
                (Arc::clone(&indexing), Arc::clone(&go), Arc::clone(&stopped));
            async move |stage: Arc<Stage>| {
                let _dropped = Dropped(stopped);
                indexing.notified().await;
                stage.set(Doing::Indexing);
                go.notified().await;
                7
            }
        };
        let soon = || Instant::now() + Duration::from_millis(50);
        assert_eq!(
            flights.join("key", "work", soon(), start).await,
            Joined::Going(Doing::Fetching)
        );
        indexing.notify_one();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(
            flights.join("key", "work", soon(), unused).await,
            Joined::Going(Doing::Indexing)
        );
        // A call that waits is called off.
        let called_off = {
            let flights = Arc::clone(&flights);
            tokio::spawn(async move { flights.join("key", "work", later(), unused).await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        called_off.abort();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!stopped.load(Ordering::SeqCst), "the work goes on");
        // Joined before the work ends, which then lets the flight go.
        let last = {
            let flights = Arc::clone(&flights);
            tokio::spawn(async move { flights.join("key", "work", later(), unused).await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        go.notify_one();
        assert_eq!(last.await.expect("joined"), Joined::Done(7));
    }

    /// The start of work that a test joins while other work for the key is in flight.
    async fn unused(_stage: Arc<Stage>) -> u32 {
        panic!("the work in flight is joined, not started")
    }
}
