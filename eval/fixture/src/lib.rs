//! The project that the eval asks about: a few items of its own, which the server builds here,
//! besides its dependencies.

/// How the service is set up.
#[derive(Debug, Clone)]
pub struct Config {
    /// The name that the service answers to.
    pub name: String,
    /// How many times a request is tried.
    retries: u32,
}

impl Config {
    /// A configuration named `name`, which tries each request three times.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            retries: 3,
        }
    }

    /// How many times a request is tried.
    #[must_use]
    pub const fn retries(&self) -> u32 {
        self.retries
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::new("service")
    }
}

/// Something that greets.
pub trait Greeter {
    /// The greeting for `who`.
    fn greet(&self, who: &str) -> String;
}

impl Greeter for Config {
    fn greet(&self, who: &str) -> String {
        format!("{} greets {who}", self.name)
    }
}

/// Calls the service once for each try that `config` allows.
pub async fn call(config: &Config) {
    for _ in 0..config.retries {
        backoff(config).await;
    }
}

/// Waits a little longer after each try, the longer the more tries `config` allows.
async fn backoff(config: &Config) {
    let millis = u64::from(config.retries).saturating_mul(10);
    tokio::time::sleep(std::time::Duration::from_millis(millis)).await;
}
