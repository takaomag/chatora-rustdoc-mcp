//! What was fetched over the network during a tool call, for its log record: which sources it
//! needed and whether the cache had everything (requirement R-OBS-1).
//!
//! Every request goes through `http::send`, which notes it here when a [`Fetches`] is in scope.

use std::{sync::Arc, time::Duration};

use parking_lot::Mutex;
use serde::Serialize;

tokio::task_local! {
    /// The fetches of the tool call that the task runs.
    static CURRENT: Fetches;
}

/// The fetches of one tool call, shared by the tasks that it spawns.
#[derive(Debug, Clone, Default)]
pub struct Fetches(Arc<Mutex<Vec<Fetch>>>);

/// One request, with its retries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Fetch {
    /// The URL without its query: `https://docs.rs/crate/tokio/1.48.0/json`.
    pub url: String,
    /// The status of the last response, or `None` when none came.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// How many times it was sent.
    pub attempts: u32,
    /// From the first send to the last response's headers, in milliseconds.
    pub ms: u64,
}

impl Fetches {
    /// Runs `future` with these fetches in scope, so that the requests it sends are noted here.
    pub async fn scope<F: Future>(self, future: F) -> F::Output {
        CURRENT.scope(self, future).await
    }

    /// Those in scope, if any, to pass on to a task that is spawned.
    #[must_use]
    pub fn current() -> Option<Self> {
        CURRENT.try_with(Clone::clone).ok()
    }

    /// The fetches noted so far.
    #[must_use]
    pub fn to_vec(&self) -> Vec<Fetch> {
        self.0.lock().clone()
    }
}

/// Notes a request that `http::send` finished with, if a [`Fetches`] is in scope.
pub(super) fn note(url: Option<&reqwest::Url>, status: Option<u16>, attempts: u32, took: Duration) {
    CURRENT
        .try_with(|fetches| {
            let url = url.map_or_else(String::new, |url| {
                let mut url = url.clone();
                url.set_query(None);
                url.set_fragment(None);
                // Credentials in the URL are not logged (requirement R-OBS-2).
                url.set_username("").unwrap_or_default();
                url.set_password(None).unwrap_or_default();
                url.into()
            });
            fetches.0.lock().push(Fetch {
                url,
                status,
                attempts,
                ms: u64::try_from(took.as_millis()).unwrap_or(u64::MAX),
            });
        })
        .unwrap_or_default();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn requests_are_noted_only_in_scope_and_without_query_or_credentials() {
        let url: reqwest::Url = "https://user:secret@docs.rs/crate/tokio/1.48.0/json?x=1#y"
            .parse()
            .expect("a URL");
        note(Some(&url), Some(200), 1, Duration::from_millis(5));
        let fetches = Fetches::default();
        fetches
            .clone()
            .scope(async {
                note(Some(&url), Some(200), 2, Duration::from_millis(5));
                assert!(Fetches::current().is_some());
            })
            .await;
        assert!(Fetches::current().is_none());
        assert_eq!(fetches.to_vec(), [Fetch {
            url: "https://docs.rs/crate/tokio/1.48.0/json".to_owned(),
            status: Some(200),
            attempts: 2,
            ms: 5,
        }]);
    }
}
