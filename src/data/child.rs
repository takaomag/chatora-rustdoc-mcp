//! Starting the children of the server (cargo, rustc and rustup) so that none of them, nor what
//! they start, outlives its use or the server (requirement R-WS-6).
//!
//! Each child runs in a group of its own with its descendants: a new session on Unix (which is
//! also a new process group, without a controlling terminal), and a Job Object on Windows.
//! Dropping a `Child` stops the whole group, so that a call that is cancelled or runs too long
//! leaves no grandchild behind (git, rustc, build scripts). [`stop_all`] stops every group, for
//! when the server ends.
//!
//! On Linux, a child is also killed when the server is (`PR_SET_PDEATHSIG`), which no handler
//! can catch. The signal comes when the *thread* that started the child ends, so every child is
//! started from a thread of its own that lives as long as the process: a thread of Tokio's
//! blocking pool ends after it has been idle for a while.

use std::{
    collections::HashMap,
    io,
    process::Output,
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
};

use parking_lot::Mutex;

/// A running child and its group, which is stopped when this is dropped.
#[derive(Debug)]
pub(super) struct Child {
    child: tokio::process::Child,
    group: StopOnDrop,
}

impl Child {
    /// Waits for the child to end and collects its stdout and stderr (piped by the caller). The
    /// rest of the group is stopped once the child has ended, and if this future is dropped first.
    ///
    /// # Errors
    ///
    /// Returns an error when the child cannot be waited for.
    pub(super) async fn output(self) -> io::Result<Output> {
        let Self { child, group } = self;
        let output = child.wait_with_output().await;
        drop(group);
        output
    }

    /// [`Self::output`], handing each line of stderr to `on_line` as it comes, as cargo's
    /// `Blocking waiting for file lock` (requirement R-WS-2). Without `\n`, the last line is
    /// handed over when stderr ends.
    ///
    /// # Errors
    ///
    /// Returns an error when the child cannot be waited for, or its output read.
    pub(super) async fn output_watching(
        self,
        mut on_line: impl FnMut(&str) + Send,
    ) -> io::Result<Output> {
        use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _};
        let Self { mut child, group } = self;
        let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
        let read_stdout = async {
            let mut read = Vec::new();
            if let Some(mut stdout) = stdout {
                stdout.read_to_end(&mut read).await?;
            }
            Ok::<_, io::Error>(read)
        };
        let read_stderr = async {
            let mut read = Vec::new();
            if let Some(stderr) = stderr {
                let mut stderr = tokio::io::BufReader::new(stderr);
                let mut line = Vec::new();
                while stderr.read_until(b'\n', &mut line).await? > 0 {
                    on_line(&String::from_utf8_lossy(&line));
                    read.append(&mut line);
                }
            }
            Ok::<_, io::Error>(read)
        };
        let (stdout, stderr, status) = tokio::try_join!(read_stdout, read_stderr, child.wait())?;
        drop(group);
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }
}

/// Stops the group under its number in [`GROUPS`] when dropped.
#[derive(Debug)]
struct StopOnDrop(u64);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        stop(self.0);
    }
}

/// The groups of the running children, by a number of this process, for [`stop_all`].
static GROUPS: OnceLock<Mutex<HashMap<u64, Group>>> = OnceLock::new();

/// Numbers the groups.
static NEXT_GROUP: AtomicU64 = AtomicU64::new(0);

/// Set by [`stop_all`]: the server is ending, and starts no more children. A call in progress
/// would otherwise go on after its child is stopped, start another, and leave it running.
static STOPPING: AtomicBool = AtomicBool::new(false);

fn groups() -> &'static Mutex<HashMap<u64, Group>> {
    GROUPS.get_or_init(Mutex::default)
}

/// Stops the group under `number`, if it is still there, and forgets it.
fn stop(number: u64) {
    let group = groups().lock().remove(&number);
    if let Some(group) = group {
        group.stop();
    }
}

/// Stops the groups of all the running children, and starts no more: for when the server ends
/// (requirement R-WS-6).
pub fn stop_all() {
    STOPPING.store(true, Ordering::SeqCst);
    let all = std::mem::take(&mut *groups().lock());
    let mut all: Vec<(u64, Group)> = all.into_iter().collect();
    all.sort_by_key(|(number, _)| *number);
    for (_, group) in all {
        group.stop();
    }
}

/// A request to the thread that starts the children.
type Start = (
    tokio::process::Command,
    tokio::runtime::Handle,
    tokio::sync::oneshot::Sender<io::Result<tokio::process::Child>>,
);

/// The thread that starts the children, which lives as long as the process.
static STARTER: OnceLock<io::Result<mpsc::Sender<Start>>> = OnceLock::new();

/// Starts `command` in a group of its own, from the thread that starts the children. The caller
/// sets the program, its arguments and its stdio; [`prepare`] has set the rest.
///
/// # Errors
///
/// Returns an error when the child cannot be started, or its group cannot be made, or the server
/// is ending.
pub(super) async fn spawn(command: tokio::process::Command) -> io::Result<Child> {
    let ending = || io::Error::other("the server is ending, so it starts nothing more");
    if STOPPING.load(Ordering::SeqCst) {
        return Err(ending());
    }
    let to_starter = STARTER
        .get_or_init(|| {
            let (sender, requests) = mpsc::channel::<Start>();
            std::thread::Builder::new()
                .name("child-starter".to_owned())
                .spawn(move || {
                    for (mut command, runtime, reply) in requests {
                        // The pipes of the child are registered with the caller's runtime.
                        let _entered = runtime.enter();
                        // The caller may have given up waiting; the child is then dropped,
                        // which kills it.
                        drop(reply.send(command.spawn()));
                    }
                })
                .map(|_| sender)
        })
        .as_ref()
        .map_err(|error| io::Error::new(error.kind(), error.to_string()))?;
    let (reply, spawned) = tokio::sync::oneshot::channel();
    to_starter
        .send((command, tokio::runtime::Handle::current(), reply))
        .map_err(|_ended| io::Error::other("the thread that starts the children has ended"))?;
    let child = spawned
        .await
        .map_err(|_ended| io::Error::other("the thread that starts the children has ended"))??;
    let group = Group::of(&child)?;
    let number = NEXT_GROUP.fetch_add(1, Ordering::Relaxed);
    groups().lock().insert(number, group);
    let child = Child {
        child,
        group: StopOnDrop(number),
    };
    // Started while `stop_all` ran: stopped here, as it may have missed it.
    if STOPPING.load(Ordering::SeqCst) {
        drop(child);
        return Err(ending());
    }
    Ok(child)
}

/// Sets up `command` to start in a group of its own: on Unix, in a new session without a
/// controlling terminal, so that git and ssh cannot open `/dev/tty` to ask for a password or a
/// host key (requirement R-WS-6), and on Linux, to be killed when the server is. The child is
/// killed when its handle is dropped, too.
pub(super) fn prepare(command: &mut tokio::process::Command) {
    command.kill_on_drop(true);
    #[cfg(unix)]
    unix::prepare(command);
}

/// A child's group: its session on Unix, its Job Object on Windows.
#[derive(Debug)]
struct Group {
    #[cfg(unix)]
    session: libc::pid_t,
    #[cfg(windows)]
    job: windows::Job,
}

impl Group {
    #[cfg(unix)]
    fn of(child: &tokio::process::Child) -> io::Result<Self> {
        let id = child
            .id()
            .ok_or_else(|| io::Error::other("the child ended before its group was known"))?;
        let session = libc::pid_t::try_from(id)
            .map_err(|_too_large| io::Error::other("a process ID out of range"))?;
        Ok(Self { session })
    }

    #[cfg(windows)]
    fn of(child: &tokio::process::Child) -> io::Result<Self> {
        let process = child
            .raw_handle()
            .ok_or_else(|| io::Error::other("the child ended before its job was made"))?;
        Ok(Self {
            job: windows::Job::with(process)?,
        })
    }

    #[cfg(not(any(unix, windows)))]
    fn of(_child: &tokio::process::Child) -> io::Result<Self> {
        Ok(Self {})
    }

    /// Kills every process of the group.
    fn stop(self) {
        #[cfg(unix)]
        unix::kill_group(self.session);
        #[cfg(windows)]
        drop(self.job);
    }
}

#[cfg(unix)]
#[expect(
    unsafe_code,
    reason = "setsid, prctl, getppid and kill are system calls without a safe wrapper in std"
)]
mod unix {
    use std::io;

    /// Runs the child in a session of its own, and on Linux, has it killed when the server is.
    pub(super) fn prepare(command: &mut tokio::process::Command) {
        #[cfg(target_os = "linux")]
        let server = libc::pid_t::try_from(std::process::id()).unwrap_or(0);
        #[cfg(target_os = "linux")]
        let (kill, unused): (libc::c_ulong, libc::c_ulong) =
            (libc::c_ulong::try_from(libc::SIGKILL).unwrap_or(9), 0);
        let in_child = move || {
            // SAFETY: setsid takes no arguments and only changes the calling process.
            if unsafe { libc::setsid() } == -1 {
                return Err(io::Error::last_os_error());
            }
            #[cfg(target_os = "linux")]
            {
                // SAFETY: PR_SET_PDEATHSIG takes the signal number as its only argument, and the
                // rest are ignored.
                let set =
                    unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, kill, unused, unused, unused) };
                if set == -1 {
                    return Err(io::Error::last_os_error());
                }
                // The server may have died before the signal was set up; then it never comes.
                // SAFETY: getppid takes no arguments and cannot fail.
                if unsafe { libc::getppid() } != server {
                    return Err(io::Error::from_raw_os_error(libc::ESRCH));
                }
            }
            Ok(())
        };
        // SAFETY: the closure runs in the forked child. It calls only setsid, prctl and getppid,
        // which are async-signal-safe, and reads errno; it neither allocates nor takes a lock.
        unsafe { command.pre_exec(in_child) };
    }

    /// Kills every process of the group `group`. A group that has no process left is not an
    /// error.
    pub(super) fn kill_group(group: libc::pid_t) {
        let Some(negative) = group.checked_neg().filter(|negative| *negative < 0) else {
            return;
        };
        // SAFETY: kill takes a process group as a negative number and a signal, and has no other
        // effect on this process.
        let killed = unsafe { libc::kill(negative, libc::SIGKILL) };
        if killed == -1 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                tracing::warn!(group, %error, "the group of a child could not be stopped");
            }
        }
    }
}

#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "Job Objects are only reached through the Windows API, which windows-sys declares \
              unsafe"
)]
mod windows {
    use std::{ffi::c_void, io, os::windows::io::RawHandle};

    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::JobObjects::{
            AssignProcessToJobObject,
            CreateJobObjectW,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectExtendedLimitInformation,
            SetInformationJobObject,
            TerminateJobObject,
        },
    };

    /// A Job Object that kills its processes when it is closed, and so when the server ends in
    /// any way. It is terminated and closed when dropped.
    #[derive(Debug)]
    pub(super) struct Job(HANDLE);

    // SAFETY: a handle of a Job Object may be used and closed from any thread.
    unsafe impl Send for Job {}

    impl Job {
        /// A new Job Object with the process `process` in it. A process that the child starts
        /// from then on is in the job too.
        pub(super) fn with(process: RawHandle) -> io::Result<Self> {
            // SAFETY: both arguments may be null: no security attributes, and no name.
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Self(handle);
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let size = u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                .map_err(|_too_large| io::Error::other("the limits do not fit"))?;
            // SAFETY: the information is the extended limits, of the size given, which live
            // across the call.
            let set = unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    std::ptr::from_ref(&limits).cast::<c_void>(),
                    size,
                )
            };
            if set == 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: both handles are open: the job here, and the process, which the caller's
            // child holds.
            if unsafe { AssignProcessToJobObject(job.0, process) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(job)
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle is open until it is closed below.
            if unsafe { TerminateJobObject(self.0, 1) } == 0 {
                tracing::warn!(error = %io::Error::last_os_error(), "the job of a child could not be stopped");
            }
            // SAFETY: the handle is open and closed only here.
            unsafe { CloseHandle(self.0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::Path,
        process::Stdio,
        time::{Duration, Instant},
    };

    use super::*;

    /// A command that starts a grandchild that runs for minutes, writes the grandchild's process
    /// ID to `pid_file`, and waits for it.
    fn with_grandchild(pid_file: &Path) -> tokio::process::Command {
        #[cfg(unix)]
        let mut command = {
            let mut command = tokio::process::Command::new("sh");
            command.args([
                "-c",
                &format!("sleep 300 & echo $! > '{}'; wait", pid_file.display()),
            ]);
            command
        };
        #[cfg(windows)]
        let mut command = {
            let mut command = tokio::process::Command::new("powershell");
            command.args([
                "-NoProfile",
                "-Command",
                &format!(
                    "$p = Start-Process -PassThru -NoNewWindow ping -ArgumentList '-n','300',\
                     '127.0.0.1'; Set-Content -Path '{}' -Value $p.Id; Wait-Process -Id $p.Id",
                    pid_file.display()
                ),
            ]);
            command
        };
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        prepare(&mut command);
        command
    }

    /// Whether the process `pid` runs.
    fn alive(pid: &str) -> bool {
        #[cfg(unix)]
        let mut check = std::process::Command::new("sh");
        #[cfg(unix)]
        check.args(["-c", &format!("kill -0 {pid} 2>/dev/null")]);
        #[cfg(windows)]
        let mut check = std::process::Command::new("powershell");
        #[cfg(windows)]
        check.args([
            "-NoProfile",
            "-Command",
            &format!(
                "if (Get-Process -Id {pid} -ErrorAction SilentlyContinue) {{ exit 0 }} exit 1"
            ),
        ]);
        check.status().expect("the check runs").success()
    }

    /// Waits up to `limit` for `done`.
    async fn wait_for(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < limit {
            if done() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        done()
    }

    #[cfg(any(unix, windows))]
    #[tokio::test]
    async fn dropping_a_child_stops_its_grandchildren() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let pid_file = dir.path().join("grandchild");
        let child = spawn(with_grandchild(&pid_file)).await.expect("started");
        let read = || {
            std::fs::read_to_string(&pid_file)
                .ok()
                .map(|pid| pid.trim().to_owned())
                .filter(|pid| !pid.is_empty())
        };
        assert!(
            wait_for(Duration::from_secs(30), || read().is_some()).await,
            "the grandchild started"
        );
        let pid = read().expect("its ID");
        assert!(alive(&pid), "the grandchild runs");
        // As when a call is cancelled or runs too long.
        let waiting = tokio::time::timeout(Duration::from_millis(100), child.output()).await;
        assert!(waiting.is_err(), "the child still runs");
        assert!(
            wait_for(Duration::from_secs(10), || !alive(&pid)).await,
            "the grandchild {pid} was stopped with the child"
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn the_child_leads_a_session_of_its_own() {
        let mut command = tokio::process::Command::new("sh");
        // The first and sixth fields of the stat are the pid and the session id.
        command
            .args(["-c", "cut -d ' ' -f 1,6 /proc/$$/stat"])
            .stdout(Stdio::piped());
        prepare(&mut command);
        let output = spawn(command)
            .await
            .expect("sh starts")
            .output()
            .await
            .expect("sh runs");
        let stat = String::from_utf8(output.stdout).expect("text");
        let (pid, session) = stat.trim().split_once(' ').expect("two fields");
        assert_eq!(pid, session, "{stat}");
    }
}
