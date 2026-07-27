//! `westonite-spawn`: client-process spawning (plan §2, §4).
//!
//! Ports `shared/process-util.c` (custom_env exec-string parsing,
//! fdstr) and the fork/exec block of `main.c` `wet_client_start`.  The
//! entire crate is the risk R-D audit surface: the `pre_exec` closure
//! runs between fork and exec and makes **only async-signal-safe
//! calls** (sigmask reset, setsid, fcntl CLOEXEC clearing — raw libc,
//! no allocation, no locking).  Nothing unwinds out of it: every
//! failure returns `Err`, which std reports to the parent through the
//! exec status pipe and turns into a failed `spawn()` (D16).

use std::ffi::OsString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command as StdCommand, Stdio};

/// The process's real uid.
///
/// Lives here because this crate already owns the process-credential
/// syscalls the frontend needs and is already the audited-unsafe one:
/// the safe `westonite` crate cannot make the call itself, and the C
/// code this crate ports needs the same value for `wet_client_launch`'s
/// `seteuid(getuid())` (main.c:469) when the helper-client path lands.
pub fn real_uid() -> u32 {
    // SAFETY: getuid is always successful, takes no arguments, and
    // touches no memory (POSIX: "shall always be successful").
    unsafe { libc::getuid() }
}

/// A CLOEXEC `AF_UNIX`/`SOCK_STREAM` socketpair for parent↔child
/// communication (C `os_socketpair_cloexec`, the frontend's
/// wayland/WM socket source).
pub fn socketpair_stream() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    // SAFETY: plain syscall writing two fds into the stack array; on
    // success both are fresh and owned by us (wrapped immediately).
    let r = unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC,
            0,
            fds.as_mut_ptr(),
        )
    };
    if r != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the kernel just handed us these two fds; nothing else
    // refers to them.
    unsafe { Ok((OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1]))) }
}

/// A CLOEXEC pipe, `(read end, write end)` (C `pipe2(O_CLOEXEC)`, the
/// frontend's Xwayland `-displayfd` readiness channel).
pub fn pipe_cloexec() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    // SAFETY: as socketpair_stream — fresh fds into a stack array.
    let r = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
    if r != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: as socketpair_stream.
    unsafe { Ok((OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1]))) }
}

/// A client command under construction.
pub struct Command {
    program: OsString,
    args: Vec<OsString>,
    env: Vec<(OsString, OsString)>,
    /// (env var name, fd) pairs: the var is set to the fd number and
    /// the fd survives exec (CLOEXEC cleared in pre_exec).
    pass_fds: Vec<(OsString, OwnedFd)>,
    /// fds referenced by number in `args` (see [`Command::arg_fd`]);
    /// they survive exec like `pass_fds` but set no env var.
    arg_fds: Vec<OwnedFd>,
}

impl Command {
    /// An empty command for `program` (argv assembled with
    /// [`Command::arg`]/[`Command::arg_fd`] — the Xwayland spawn path).
    pub fn new(program: impl Into<OsString>) -> Command {
        Command {
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
            pass_fds: Vec::new(),
            arg_fds: Vec::new(),
        }
    }

    /// From an argv list (CLI autolaunch trailing args).
    pub fn from_argv(argv: &[String]) -> Option<Command> {
        let (program, args) = argv.split_first()?;
        Some(Command {
            program: program.into(),
            args: args.iter().map(Into::into).collect(),
            env: Vec::new(),
            pass_fds: Vec::new(),
            arg_fds: Vec::new(),
        })
    }

    /// From an exec string: leading `VAR=value` words become child env
    /// entries, the rest is whitespace-split into argv (the C
    /// custom_env `ENV=x cmd arg` contract; whitespace splitting only,
    /// no quoting — same as the C parser).
    ///
    /// Two **deliberate divergences** from
    /// `custom_env_add_from_exec_string`, both narrowing what counts as
    /// an assignment: C stops at the first `=` in the word with no
    /// further checks, so `/opt/w=x/app` sets an env var named `/opt/w`
    /// and `=v cmd` sets one named `""`.  Neither is reachable through
    /// a useful config, and both produce a child that fails in a
    /// confusing way, so a word whose key contains `/` or is empty ends
    /// the env prefix here instead.
    pub fn from_exec_string(s: &str) -> Option<Command> {
        let mut env = Vec::new();
        let mut words = s.split_whitespace().peekable();
        while let Some(w) = words.peek() {
            match w.split_once('=') {
                // A `=` in the word marks an env assignment, unless the
                // key looks like a path or is empty (see above).
                Some((k, v)) if !k.contains('/') && !k.is_empty() => {
                    env.push((OsString::from(k), OsString::from(v)));
                    words.next();
                }
                _ => break,
            }
        }
        let program: OsString = words.next()?.into();
        Some(Command {
            program,
            args: words.map(Into::into).collect(),
            env,
            pass_fds: Vec::new(),
            arg_fds: Vec::new(),
        })
    }

    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Command {
        self.env.push((key.into(), value.into()));
        self
    }

    pub fn arg(mut self, arg: impl Into<OsString>) -> Command {
        self.args.push(arg.into());
        self
    }

    /// Pass an fd by argv position: appends the decimal fd number as
    /// the next argument (the C fdstr contract for `-listenfd N`,
    /// `-displayfd N`, `-wm N`) and keeps the fd alive across exec
    /// (CLOEXEC cleared after fork).  The number is stable: the
    /// `OwnedFd` is held until exec, and fork changes no fd numbers.
    pub fn arg_fd(mut self, fd: OwnedFd) -> Command {
        self.args.push(fd.as_raw_fd().to_string().into());
        self.arg_fds.push(fd);
        self
    }

    /// Pass an fd to the child: `key` is set to the decimal fd number
    /// (the C fdstr contract, e.g. WAYLAND_SOCKET) and the fd's
    /// CLOEXEC flag is cleared after fork.
    pub fn pass_fd(mut self, key: impl Into<OsString>, fd: OwnedFd) -> Command {
        self.pass_fds.push((key.into(), fd));
        self
    }

    /// Fork and exec.  Mirrors wet_client_start: the child gets a fresh
    /// session (setsid), a cleared signal mask, and the passed fds.
    pub fn spawn(self) -> io::Result<Child> {
        let mut cmd = StdCommand::new(&self.program);
        cmd.args(&self.args);
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        let mut raw_fds = Vec::with_capacity(self.pass_fds.len() + self.arg_fds.len());
        for (k, fd) in &self.pass_fds {
            cmd.env(k, fd.as_raw_fd().to_string());
            raw_fds.push(fd.as_raw_fd());
        }
        for fd in &self.arg_fds {
            raw_fds.push(fd.as_raw_fd());
        }
        cmd.stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        // SAFETY: (risk R-D, the crate's whole audit) this closure runs
        // in the forked child before exec.  Every call below is
        // async-signal-safe (raw syscalls via libc: sigprocmask with a
        // stack sigset, setsid, fcntl) — no allocation, no locks, no
        // unwinding (errors return, and std turns a pre_exec error into
        // child exit, never unwind).
        unsafe {
            cmd.pre_exec(move || {
                // Reset the signal mask (the compositor blocks SIGCHLD
                // etc.; clients must start clean — C child_client_exec).
                let mut set: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut set);
                if libc::sigprocmask(libc::SIG_SETMASK, &set, std::ptr::null_mut()) != 0 {
                    return Err(io::Error::last_os_error());
                }
                // New session: the client must not die with our TTY.
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                // Passed fds survive exec.
                for fd in &raw_fds {
                    let flags = libc::fcntl(*fd, libc::F_GETFD);
                    if flags == -1
                        || libc::fcntl(*fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) == -1
                    {
                        return Err(io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }

        let child = cmd.spawn()?;
        // Parent side: drop our copies of the passed fds now that the
        // child owns them (OwnedFd drop closes).
        drop(self.pass_fds);
        drop(self.arg_fds);
        Ok(child)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn exec_string_parses_env_prefix() {
        let c = Command::from_exec_string("FOO=1 BAR=two /bin/echo hi there").unwrap();
        assert_eq!(c.program, OsString::from("/bin/echo"));
        assert_eq!(c.args, vec![OsString::from("hi"), OsString::from("there")]);
        assert_eq!(
            c.env,
            vec![
                (OsString::from("FOO"), OsString::from("1")),
                (OsString::from("BAR"), OsString::from("two"))
            ]
        );
    }

    #[test]
    fn exec_string_path_with_equals_is_not_env() {
        // Deliberate divergence from the C parser (see from_exec_string):
        // a path containing '=' is not eaten as an assignment.
        let c = Command::from_exec_string("/opt/w=x/app --flag").unwrap();
        assert_eq!(c.program, OsString::from("/opt/w=x/app"));
        assert!(c.env.is_empty());
    }

    #[test]
    fn empty_inputs() {
        assert!(Command::from_argv(&[]).is_none());
        assert!(Command::from_exec_string("   ").is_none());
        assert!(Command::from_exec_string("FOO=1").is_none());
    }

    #[test]
    fn spawn_runs_with_env_and_clean_session() {
        let out = tempfile_path();
        let c = Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("echo -n $MARK > {out}"))
            .env("MARK", "yes");
        let mut child = c.spawn().unwrap();
        assert!(child.wait().unwrap().success());
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "yes");
        let _ = std::fs::remove_file(&out);
    }

    fn tempfile_path() -> String {
        let p = std::env::temp_dir().join(format!("wspawn-test-{}", std::process::id()));
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn arg_fd_survives_exec_by_number() {
        // The child writes to the pipe write-end whose NUMBER it got as
        // an argv word — the -displayfd contract.
        let (read, write) = pipe_cloexec().unwrap();
        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg("echo done >&$1; exit 0")
            .arg("sh")
            .arg_fd(write)
            .spawn()
            .unwrap();
        assert!(child.wait().unwrap().success());
        let mut buf = [0u8; 16];
        // SAFETY-free read via File wrapper.
        let n = {
            use std::io::Read;
            let mut f = std::fs::File::from(read);
            f.read(&mut buf).unwrap()
        };
        assert_eq!(&buf[..n], b"done\n");
    }

    #[test]
    fn socketpair_ends_are_connected() {
        let (a, b) = socketpair_stream().unwrap();
        let mut sa = std::os::unix::net::UnixStream::from(a);
        let mut sb = std::os::unix::net::UnixStream::from(b);
        use std::io::{Read, Write};
        sa.write_all(b"ping").unwrap();
        let mut buf = [0u8; 4];
        sb.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"ping");
    }
}
