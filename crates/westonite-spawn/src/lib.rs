//! `westonite-spawn`: client-process spawning (plan §2, §4).
//!
//! Ports `shared/process-util.c` (custom_env exec-string parsing,
//! fdstr) and the fork/exec block of `main.c` `wet_client_start`.  The
//! entire crate is the risk R-D audit surface: the `pre_exec` closure
//! runs between fork and exec and makes **only async-signal-safe
//! calls** (sigmask reset, setsid, fcntl CLOEXEC clearing — raw libc,
//! no allocation, no locking).  The child aborts rather than unwinds on
//! any failure there (D16).

use std::ffi::OsString;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command as StdCommand, Stdio};

/// A client command under construction.
pub struct Command {
    program: OsString,
    args: Vec<OsString>,
    env: Vec<(OsString, OsString)>,
    /// (env var name, fd) pairs: the var is set to the fd number and
    /// the fd survives exec (CLOEXEC cleared in pre_exec).
    pass_fds: Vec<(OsString, OwnedFd)>,
}

impl Command {
    /// From an argv list (CLI autolaunch trailing args).
    pub fn from_argv(argv: &[String]) -> Option<Command> {
        let (program, args) = argv.split_first()?;
        Some(Command {
            program: program.into(),
            args: args.iter().map(Into::into).collect(),
            env: Vec::new(),
            pass_fds: Vec::new(),
        })
    }

    /// From an exec string: leading `VAR=value` words become child env
    /// entries, the rest is whitespace-split into argv (the C
    /// custom_env `ENV=x cmd arg` contract; whitespace splitting only,
    /// no quoting — same as the C parser).
    pub fn from_exec_string(s: &str) -> Option<Command> {
        let mut env = Vec::new();
        let mut words = s.split_whitespace().peekable();
        while let Some(w) = words.peek() {
            match w.split_once('=') {
                // A `=` before any `/` in the word marks an env
                // assignment (C: strchr(w, '=') before the command).
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
        })
    }

    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Command {
        self.env.push((key.into(), value.into()));
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
        let mut raw_fds = Vec::with_capacity(self.pass_fds.len());
        for (k, fd) in &self.pass_fds {
            cmd.env(k, fd.as_raw_fd().to_string());
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
        // A path containing '=' must not be eaten as an assignment.
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
        let c = Command {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), format!("echo -n $MARK > {out}").into()],
            env: vec![(OsString::from("MARK"), OsString::from("yes"))],
            pass_fds: Vec::new(),
        };
        let mut child = c.spawn().unwrap();
        assert!(child.wait().unwrap().success());
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "yes");
        let _ = std::fs::remove_file(&out);
    }

    fn tempfile_path() -> String {
        let p = std::env::temp_dir().join(format!("wspawn-test-{}", std::process::id()));
        p.to_string_lossy().into_owned()
    }
}
