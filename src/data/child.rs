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
    path::Path,
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

/// What a request to the thread that starts the children keeps until the child has started, and
/// hands back: on Unix, the child's record, whose lock the child inherits by the number of its
/// file descriptor, which must not be closed and used again meanwhile (decided in C4).
#[cfg(unix)]
type Kept = Option<super::owner::ChildRecord>;
#[cfg(not(unix))]
type Kept = ();

/// A request to the thread that starts the children.
type Start = (
    tokio::process::Command,
    tokio::runtime::Handle,
    Kept,
    tokio::sync::oneshot::Sender<(io::Result<tokio::process::Child>, Kept)>,
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
    spawn_owned(command, super::owner::Owner::this(), None).await
}

/// [`spawn`] for a build in `target_dir`, a target directory in the cache, which its record names
/// (decided in M2's D2): a later server's sweep marks it as not to be used when what outlives the
/// server cannot be told for sure to be the child's.
///
/// # Errors
///
/// As [`spawn`].
#[expect(dead_code, reason = "the build of M2's D2b starts its children so")]
pub(super) async fn spawn_building(
    command: tokio::process::Command,
    target_dir: &Path,
) -> io::Result<Child> {
    spawn_owned(command, super::owner::Owner::this(), Some(target_dir)).await
}

/// [`spawn`], recording the child for `owner`, if any, on Unix (requirement R-WS-6, decided in
/// C4): its record, and a lock that its descendants inherit.
async fn spawn_owned(
    #[cfg_attr(
        not(unix),
        expect(unused_mut, reason = "only Unix adds to the command")
    )]
    mut command: tokio::process::Command,
    #[cfg_attr(
        not(unix),
        expect(unused_variables, reason = "only Unix records children")
    )]
    owner: Option<&super::owner::Owner>,
    #[cfg_attr(
        not(unix),
        expect(unused_variables, reason = "only Unix records children")
    )]
    target_dir: Option<&Path>,
) -> io::Result<Child> {
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
                    for (mut command, runtime, kept, reply) in requests {
                        // The pipes of the child are registered with the caller's runtime.
                        let _entered = runtime.enter();
                        // The caller may have given up waiting; the child is then dropped,
                        // which kills it, and what was kept for it after it has started.
                        drop(reply.send((command.spawn(), kept)));
                    }
                })
                .map(|_| sender)
        })
        .as_ref()
        .map_err(|error| io::Error::new(error.kind(), error.to_string()))?;
    let number = NEXT_GROUP.fetch_add(1, Ordering::Relaxed);
    #[cfg(unix)]
    let record = owner.and_then(|owner| match owner.child_record(number, target_dir) {
        Ok(record) => Some(record),
        Err(error) => {
            tracing::warn!(%error, "a child is started without its record");
            None
        },
    });
    #[cfg(unix)]
    if let Some(fd) = record
        .as_ref()
        .and_then(super::owner::ChildRecord::inherited)
    {
        unix::inherit(&mut command, fd);
    }
    #[cfg(not(unix))]
    let record = ();
    let (reply, spawned) = tokio::sync::oneshot::channel();
    to_starter
        .send((command, tokio::runtime::Handle::current(), record, reply))
        .map_err(|_ended| io::Error::other("the thread that starts the children has ended"))?;
    #[cfg_attr(
        not(unix),
        expect(unused_variables, reason = "only Unix records children")
    )]
    let (child, record) = spawned
        .await
        .map_err(|_ended| io::Error::other("the thread that starts the children has ended"))?;
    let child = child?;
    #[cfg_attr(not(unix), expect(unused_mut, reason = "only Unix records children"))]
    let mut group = Group::of(&child)?;
    #[cfg(unix)]
    if let Some(record) = record {
        record.started(group.session.unsigned_abs());
        group.record = Some(record);
    }
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
    /// The record of the child and the lock that its descendants inherit, which go once the
    /// group is stopped and none of it runs (decided in C4).
    #[cfg(unix)]
    record: Option<super::owner::ChildRecord>,
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
        Ok(Self {
            session,
            record: None,
        })
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
        // Dropped after the kill, which then tells whether a descendant still runs.
        #[cfg(unix)]
        drop(self.record);
        #[cfg(windows)]
        drop(self.job);
    }
}

#[cfg(unix)]
#[expect(
    unsafe_code,
    reason = "setsid, prctl, getppid, fcntl and kill are system calls without a safe wrapper in \
              std"
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

    /// Has the child inherit the file descriptor `fd`, which this process opened close-on-exec:
    /// the lock on it is then held while the child or any of its descendants runs (decided in
    /// C4).
    pub(super) fn inherit(command: &mut tokio::process::Command, fd: std::os::fd::RawFd) {
        let in_child = move || {
            // SAFETY: F_SETFD with 0 clears close-on-exec on `fd`, which the child has as the
            // server's copy; it changes nothing else.
            if unsafe { libc::fcntl(fd, libc::F_SETFD, 0) } == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        };
        // SAFETY: the closure runs in the forked child, and only calls fcntl, which is
        // async-signal-safe, and reads errno.
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

    /// Whether the process `pid` runs, not as a zombie: one that was killed and waits for this
    /// process to take its status still answers `kill -0`.
    #[cfg(unix)]
    fn running(pid: &str) -> bool {
        let output = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", pid])
            .output()
            .expect("ps runs");
        let state = String::from_utf8_lossy(&output.stdout);
        let state = state.trim();
        !state.is_empty() && !state.starts_with('Z')
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

    /// A child is recorded while it runs, with its session, when it started and the target
    /// directory that it builds in, and once it has ended with nothing of its group left, neither
    /// its record nor its lock stays (decided in C4 and D2).
    #[cfg(unix)]
    #[tokio::test]
    async fn a_child_is_recorded_while_it_runs() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let owner = super::super::owner::Owner::start(&super::super::cache::Cache::at(dir.path()));
        let mut command = tokio::process::Command::new("sh");
        command.args(["-c", "sleep 0.5"]).stdin(Stdio::null());
        prepare(&mut command);
        let target_dir = dir.path().join("t/0123456789ab.1");
        let child = spawn_owned(command, Some(&owner), Some(&target_dir))
            .await
            .expect("started");
        let pid = child.child.id().expect("running");
        let records = dir.path().join("procs/v1");
        let record = records.join(format!("{}.{}.json", owner.id(), child.group.0));
        let written: super::super::owner::Record =
            serde_json::from_slice(&std::fs::read(&record).expect("recorded")).expect("a record");
        assert_eq!(written.session, pid);
        assert_eq!(written.started, super::super::process_id::started(pid));
        assert!(written.started.is_some());
        assert_eq!(written.target_dir.as_deref(), Some("t/0123456789ab.1"));
        assert_eq!(
            written.boot.as_ref(),
            super::super::process_id::machine_and_boot().map(|(_, boot)| boot)
        );
        child.output().await.expect("ran");
        let left: Vec<_> = std::fs::read_dir(&records)
            .expect("the directory")
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        assert!(left.is_empty(), "{left:?}");
    }

    /// A descendant that leaves the child's session is not stopped with the group, but it holds
    /// the lock that it inherited, so the record stays, for the sweep of a later server (decided
    /// in C4).
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn the_record_stays_while_a_descendant_runs() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let owner = super::super::owner::Owner::start(&super::super::cache::Cache::at(dir.path()));
        let pid_file = dir.path().join("descendant");
        let mut command = tokio::process::Command::new("sh");
        command
            .args([
                "-c",
                // The shell in the new session writes its own pid (`setsid` may fork first), and
                // the child waits for it, so that the descendant has left the group before the
                // group is stopped.
                &format!(
                    "setsid sh -c 'echo $$ > \"{0}\"; exec sleep 60' >/dev/null 2>&1 & \
                     while [ ! -s '{0}' ]; do sleep 0.05; done",
                    pid_file.display()
                ),
            ])
            .stdin(Stdio::null());
        prepare(&mut command);
        let child = spawn_owned(command, Some(&owner), None)
            .await
            .expect("started");
        let number = child.group.0;
        child.output().await.expect("ran");
        let read = || {
            std::fs::read_to_string(&pid_file)
                .ok()
                .map(|pid| pid.trim().to_owned())
                .filter(|pid| !pid.is_empty())
        };
        assert!(
            wait_for(Duration::from_secs(10), || read().is_some()).await,
            "the descendant started"
        );
        let descendant = read().expect("its pid");
        let record = dir
            .path()
            .join(format!("procs/v1/{}.{number}.json", owner.id()));
        let kept = record.exists();
        let ran = alive(&descendant);
        // Stopped here, as the sweep of C4c would.
        let killed = std::process::Command::new("kill")
            .args(["-9", &descendant])
            .status()
            .expect("kill runs");
        assert!(
            ran && killed.success(),
            "{descendant} ran outside the group"
        );
        assert!(kept, "the record is kept while {descendant} runs");
    }

    /// Later than any test's deadline.
    #[cfg(unix)]
    fn later() -> Instant {
        Instant::now()
            .checked_add(Duration::from_secs(60))
            .expect("in range")
    }

    /// The environment variable that tells [`start_a_child_for_another_test`] the cache.
    #[cfg(unix)]
    const ROOT: &str = "CHATORA_RUSTDOC_TEST_CACHE_ROOT";

    /// Not a test by itself: a server that
    /// [`a_later_server_stops_what_a_killed_server_left`] starts and kills, which starts a child
    /// with a grandchild, writes the grandchild's pid in the cache, and waits.
    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "started by a_later_server_stops_what_a_killed_server_left"]
    async fn start_a_child_for_another_test() {
        let Some(root) = std::env::var_os(ROOT) else {
            return;
        };
        let root = Path::new(&root);
        let owner = super::super::owner::Owner::start(&super::super::cache::Cache::at(root));
        let _child = spawn_owned(
            with_grandchild(&root.join("grandchild")),
            Some(&owner),
            None,
        )
        .await
        .expect("started");
        tokio::time::sleep(Duration::from_secs(120)).await;
    }

    /// The sweep of a later server stops what a child of a killed server left running, which is
    /// its descendants for sure, and removes the record (decided in C4).
    #[cfg(unix)]
    #[tokio::test]
    async fn a_later_server_stops_what_a_killed_server_left() {
        use super::super::{cache::Cache, owner};
        let dir = tempfile::tempdir().expect("a temporary directory");
        let mut killed = crate::test_process::start(
            "data::child::tests::start_a_child_for_another_test",
            &[(ROOT, dir.path().as_os_str())],
        );
        let pid_file = dir.path().join("grandchild");
        let read = || {
            std::fs::read_to_string(&pid_file)
                .ok()
                .filter(|pid| pid.ends_with('\n'))
                .map(|pid| pid.trim().to_owned())
        };
        assert!(
            wait_for(Duration::from_secs(30), || read().is_some()).await,
            "the grandchild started"
        );
        let grandchild = read().expect("its pid");
        killed.kill().expect("killed");
        killed.wait().expect("waited");
        let me = owner::Owner::start(&Cache::at(dir.path()));
        let swept = owner::sweep(&me, later());
        assert_eq!(swept.child_records, 1, "{swept:?}");
        assert!(
            swept.stopped >= 1,
            "the grandchild, and on macOS the child: {swept:?}"
        );
        assert!(
            wait_for(Duration::from_secs(10), || !alive(&grandchild)).await,
            "the grandchild {grandchild} was stopped"
        );
        let left: Vec<_> = std::fs::read_dir(dir.path().join("procs/v1"))
            .expect("the directory")
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        assert!(left.is_empty(), "{left:?}");
    }

    /// What runs in the recorded session is left alone when it cannot be told for sure to be the
    /// child's: a record of another boot, or a session whose leader started at another time, as
    /// when its ID was used again (decided in C4).
    #[cfg(unix)]
    #[tokio::test]
    async fn what_cannot_be_told_for_sure_is_left() {
        use super::super::{
            cache::Cache,
            owner::{self, Record},
            process_id,
            process_lock::{self, LockMode},
        };
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let gone = owner::Owner::start(&cache);
        gone.killed_for_test();
        // Another process in a session of its own, which a record names.
        let mut command = tokio::process::Command::new("sleep");
        command.arg("60").stdin(Stdio::null());
        prepare(&mut command);
        let other = spawn_owned(command, None, None).await.expect("started");
        let pid = other.child.id().expect("running");
        let since = process_id::started(pid).expect("its start");
        let (machine, boot) = process_id::machine_and_boot()
            .cloned()
            .map_or((None, None), |(machine, boot)| (Some(machine), Some(boot)));
        let records = dir.path().join("procs/v1");
        std::fs::create_dir_all(&records).expect("made");
        let target_dir = dir.path().join("t/0123456789ab");
        std::fs::create_dir_all(&target_dir).expect("made");
        let mut held = Vec::new();
        // On Linux, a process that matches the record but has not the inherited lock open is
        // not the child's.
        #[cfg(target_os = "linux")]
        let matching = Some(Record {
            machine: machine.clone(),
            boot: boot.clone(),
            session: pid,
            started: Some(since),
            target_dir: None,
        });
        #[cfg(not(target_os = "linux"))]
        let matching: Option<Record> = None;
        for (number, record) in [
            // The target directory that this one built in is not to be used.
            Record {
                machine: machine.clone(),
                boot: Some("another boot".to_owned()),
                session: pid,
                started: Some(since),
                target_dir: Some("t/0123456789ab".to_owned()),
            },
            // Nor anything that is not a target directory of the cache.
            Record {
                machine,
                boot,
                session: pid,
                started: Some(since.saturating_sub(1)),
                target_dir: Some("t/../work".to_owned()),
            },
        ]
        .into_iter()
        .chain(matching)
        .enumerate()
        {
            let stem = format!("{}.{number}", gone.id());
            std::fs::write(
                records.join(format!("{stem}.json")),
                serde_json::to_vec(&record).expect("JSON"),
            )
            .expect("written");
            // A descendant holds the lock.
            held.push(
                process_lock::try_lock_at(
                    &records.join(format!("{stem}.lock")),
                    LockMode::Exclusive,
                )
                .expect("locked")
                .expect("free"),
            );
        }
        let me = owner::Owner::start(&cache);
        let swept = owner::sweep(&me, later());
        assert_eq!(swept.unsure, held.len(), "{swept:?}");
        assert_eq!(swept.stopped, 0, "{swept:?}");
        assert!(running(&pid.to_string()), "left alone");
        assert!(target_dir.join("unusable").exists(), "marked");
        assert!(!dir.path().join("work/unusable").exists(), "not marked");
        drop(held);
        drop(other);
    }
}
