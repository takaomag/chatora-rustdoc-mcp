//! What a tool call waits for, which its progress notifications tell (requirement R-WS-6).
//!
//! [`super::flight::Flights::join`] notes the work it waits for here when a [`Waits`] is in scope.
//! The work is shared, so each call that waits for it tells its own progress.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use parking_lot::Mutex;

use super::flight::Stage;

tokio::task_local! {
    /// What the tool call that the task runs waits for.
    static CURRENT: Waits;
}

/// What one tool call waits for, shared by the tasks that it spawns.
#[derive(Debug, Clone, Default)]
pub struct Waits(Arc<Mutex<Vec<Wait>>>);

/// Work that a call waits for.
#[derive(Debug)]
struct Wait {
    /// Tells it from the others.
    id: u64,
    /// What the work makes, as `serde 1.0.228`.
    what: Box<str>,
    /// What the work does now.
    stage: Arc<Stage>,
}

impl Waits {
    /// Runs `future` with these waits in scope, so that the work it waits for is noted here.
    pub async fn scope<F: Future>(self, future: F) -> F::Output {
        CURRENT.scope(self, future).await
    }

    /// Those in scope, if any, to pass on to a task that is spawned.
    #[must_use]
    pub fn current() -> Option<Self> {
        CURRENT.try_with(Clone::clone).ok()
    }

    /// What the call waits for, for people to read: `fetching serde 1.0.228`, or with more than
    /// one piece of work, how many and the first. `None` when it waits for none.
    #[must_use]
    pub fn message(&self) -> Option<String> {
        let waits = self.0.lock();
        let first = waits.first()?;
        let first = format!("{} {}", first.stage.get().as_words(), first.what);
        Some(match waits.len() {
            1 => first,
            many => format!("loading {many} crates: {first}, …"),
        })
    }
}

/// Notes, while the guard lives, that the call in scope waits for work on `what`, which does what
/// `stage` says. Nothing is noted outside a call.
pub(super) fn waiting(what: &str, stage: &Arc<Stage>) -> Option<Waiting> {
    static IDS: AtomicU64 = AtomicU64::new(0);
    let waits = Waits::current()?;
    let id = IDS.fetch_add(1, Ordering::Relaxed);
    waits.0.lock().push(Wait {
        id,
        what: what.into(),
        stage: Arc::clone(stage),
    });
    Some(Waiting { waits, id })
}

/// The note that a call waits for work, taken back when it is dropped.
#[derive(Debug)]
pub(super) struct Waiting {
    waits: Waits,
    id: u64,
}

impl Drop for Waiting {
    fn drop(&mut self) {
        self.waits.0.lock().retain(|wait| wait.id != self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::flight::Doing;

    #[tokio::test]
    async fn the_message_tells_what_the_call_waits_for_while_it_waits() {
        let waits = Waits::default();
        let stage = Arc::new(Stage::default());
        waits
            .clone()
            .scope(async {
                let first = waiting("serde 1.0.228", &stage).expect("in a call");
                assert_eq!(waits.message().as_deref(), Some("fetching serde 1.0.228"));
                stage.set(Doing::Indexing);
                assert_eq!(waits.message().as_deref(), Some("indexing serde 1.0.228"));
                let second = waiting("tokio 1.48.0", &Arc::new(Stage::default()));
                assert_eq!(
                    waits.message().as_deref(),
                    Some("loading 2 crates: indexing serde 1.0.228, …")
                );
                drop(first);
                assert_eq!(waits.message().as_deref(), Some("fetching tokio 1.48.0"));
                drop(second);
            })
            .await;
        assert_eq!(waits.message(), None);
        assert!(waiting("serde 1.0.228", &stage).is_none(), "outside a call");
    }
}
