//! The lines of the log file, written on a thread of their own, so that a thread that logs, such
//! as one that runs async work, does not wait for the disk (requirement R-OBS-2).

use std::{
    io,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, SyncSender, TrySendError},
    time::{Duration, Instant},
};

use super::file::LogFile;

/// How many lines may wait for the writer: past them, as when the disk is stuck, a thread that
/// logs waits, rather than the lines being lost.
const QUEUE: usize = 4096;

/// What the writer is sent.
enum Message {
    /// A line to write.
    Line(String),
    /// Told back once the lines before it are written.
    Flush(SyncSender<()>),
}

/// The lines for a [`LogFile`], which a thread of its own writes.
#[derive(Debug)]
pub(super) struct Lines {
    sender: SyncSender<Message>,
    /// The file that was being written when the writer started.
    path: PathBuf,
}

impl Lines {
    /// Starts the thread that writes the lines to `file`, and goes on to new files and removes
    /// old ones as it does.
    ///
    /// # Errors
    ///
    /// Returns an error when the thread cannot be started.
    pub(super) fn start(file: LogFile) -> io::Result<Self> {
        let path = file.path();
        let (sender, receiver) = mpsc::sync_channel(QUEUE);
        std::thread::Builder::new()
            .name("log-writer".to_owned())
            .spawn(move || write(&file, &receiver))?;
        Ok(Self { sender, path })
    }

    /// The file that was being written when the writer started.
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    /// Hands `line` to the writer, waiting only while it is [`QUEUE`] lines behind.
    pub(super) fn write_line(&self, line: String) {
        if self.sender.send(Message::Line(line)).is_err() {
            gone();
        }
    }

    /// Waits at most `limit` for the lines handed over so far to be written, as when the process
    /// ends.
    pub(super) fn flush(&self, limit: Duration) {
        let start = Instant::now();
        let (done, written) = mpsc::sync_channel(1);
        let mut flush = Message::Flush(done);
        loop {
            match self.sender.try_send(flush) {
                Ok(()) => break,
                Err(TrySendError::Full(again)) if start.elapsed() < limit => {
                    flush = again;
                    std::thread::sleep(Duration::from_millis(10));
                },
                Err(TrySendError::Full(_)) => {
                    eprintln!(
                        "{}: the log file was not written to its end in time",
                        env!("CARGO_PKG_NAME")
                    );
                    return;
                },
                Err(TrySendError::Disconnected(_)) => {
                    gone();
                    return;
                },
            }
        }
        if written
            .recv_timeout(limit.saturating_sub(start.elapsed()))
            .is_err()
        {
            eprintln!(
                "{}: the log file was not written to its end in time",
                env!("CARGO_PKG_NAME")
            );
        }
    }
}

/// Tells once, on stderr, that the writer is gone, as when it panicked: logging cannot tell it
/// to itself.
fn gone() {
    static TOLD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !TOLD.swap(true, std::sync::atomic::Ordering::Relaxed) {
        eprintln!(
            "{}: the thread that writes the log file has stopped; lines are lost",
            env!("CARGO_PKG_NAME")
        );
    }
}

/// Writes what `receiver` gets to `file`, until every sender is gone.
fn write(file: &LogFile, receiver: &Receiver<Message>) {
    for message in receiver {
        match message {
            Message::Line(line) => file.write_line(&line),
            Message::Flush(done) => drop(done.send(())),
        }
    }
}
