//! Programs that share a socket with a child, or start an approved program on it
//! (`docs/sdk.md`, "Security"). These tests fork. A child gets a copy of each socket
//! of the test program, so they are in a test program of their own: the socket of
//! another test would stay open until the child ends.

#![cfg(any(target_os = "macos", target_os = "linux"))]

use sayso_host::Caller;
use std::path::PathBuf;
use std::time::Duration;

mod common;
use common::*;

/// A program connects and gives the socket to a child. The child writes, and
/// the program starts an approved program, with the real checks of the system.
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod shared_socket {
    use super::*;
    use sayso_host::{HostConfig, Identify, Peer, SystemIdentify};
    use std::ffi::CString;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::path::Path;
    use std::sync::Arc;

    const HELLO: &[u8] = b"{\"v\":1,\"type\":\"hello\",\"id\":\"h1\",\"protocol\":1}\n";

    /// The check comes after the exec, as when the queue of accepts is long.
    struct LateIdentify;

    impl Identify for LateIdentify {
        fn identify(&self, peer: &Peer) -> Result<Caller, String> {
            std::thread::sleep(Duration::from_millis(500));
            SystemIdentify.identify(peer)
        }
    }

    /// `/bin/sleep` is approved, and the test program is refused.
    fn approved_program(setup: &Setup) -> PathBuf {
        let approved = Path::new("/bin/sleep").canonicalize().unwrap();
        let text = format!("[[apps]]\nname = \"Target\"\npath = '{}'\nallowed = true\n\n[[apps]]\nname = \"Test\"\npath = '{}'\nallowed = false\n", approved.display(), this_program().display());
        std::fs::write(&setup.apps, text).unwrap();
        approved
    }

    fn start_late(setup: &Setup, services: Arc<FakeServices>) -> sayso_host::Host {
        let mut config = HostConfig::new(setup.socket.clone(), setup.apps.clone());
        config.identify = Arc::new(LateIdentify);
        sayso_host::Host::start(config, services).unwrap()
    }

    /// Fork a process that connects and starts `approved`, with a child that
    /// keeps the socket. Who writes `HELLO`: the child after the exec, or the
    /// process before it. Gives what the child reads.
    fn exec_with_a_child(socket: &Path, approved: &Path, process_writes: bool) -> String {
        let program = CString::new(approved.as_os_str().as_encoded_bytes()).unwrap();
        let (name, time) = (c"sleep", c"30");
        let arguments = [name.as_ptr(), time.as_ptr(), std::ptr::null()];
        let mut pipe = [0; 2];
        // SAFETY: after each fork, the child makes only calls that are safe
        // after a fork of a process with threads, on buffers made before it.
        unsafe {
            assert_eq!(libc::pipe(pipe.as_mut_ptr()), 0);
            let process = libc::fork();
            assert!(process >= 0);
            if process == 0 {
                let Ok(stream) = UnixStream::connect(socket) else { libc::_exit(1) };
                let fd = std::os::fd::IntoRawFd::into_raw_fd(stream);
                // The approved program keeps the socket, as in the attack.
                libc::fcntl(fd, libc::F_SETFD, 0);
                if process_writes {
                    libc::write(fd, HELLO.as_ptr().cast(), HELLO.len());
                }
                if libc::fork() == 0 {
                    if !process_writes {
                        // The exec is over by now, and the check comes later.
                        libc::usleep(200_000);
                        libc::write(fd, HELLO.as_ptr().cast(), HELLO.len());
                    }
                    let mut reply = [0u8; 512];
                    let count = libc::read(fd, reply.as_mut_ptr().cast(), reply.len());
                    if count > 0 {
                        libc::write(pipe[1], reply.as_ptr().cast(), count as usize);
                    }
                    libc::_exit(0);
                }
                libc::close(pipe[1]);
                libc::execv(program.as_ptr(), arguments.as_ptr());
                libc::_exit(1);
            }
            libc::close(pipe[1]);
            let mut reader = <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(pipe[0]);
            let mut reply = String::new();
            reader.read_to_string(&mut reply).unwrap();
            libc::kill(process, libc::SIGKILL);
            libc::waitpid(process, std::ptr::null_mut(), 0);
            reply
        }
    }

    #[test]
    fn a_child_that_writes_after_an_exec_of_its_parent_is_not_the_approved_program() {
        let setup = setup();
        let approved = approved_program(&setup);
        let services = FakeServices::new(None);
        let _host = start_late(&setup, services.clone());
        let reply = exec_with_a_child(&setup.socket, &approved, false);
        assert!(reply.contains("not_approved"), "{reply}");
        assert!(services.approvals.lock().is_empty());
    }

    /// The token of the line has the version from before the exec. On Linux,
    /// this case is open: "Security" in `docs/sdk.md`.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_hello_from_before_the_exec_does_not_count_for_the_approved_program() {
        let setup = setup();
        let approved = approved_program(&setup);
        let services = FakeServices::new(None);
        let _host = start_late(&setup, services.clone());
        let reply = exec_with_a_child(&setup.socket, &approved, true);
        assert!(reply.contains("not_approved"), "{reply}");
        assert!(services.approvals.lock().is_empty());
    }

    #[test]
    fn a_line_from_another_process_closes_the_connection() {
        let setup = setup();
        write_apps(&setup.apps, &[]);
        let host = start_with(&setup, FakeServices::new(None));
        let mut stream = UnixStream::connect(&setup.socket).unwrap();
        let mut lines = BufReader::new(stream.try_clone().unwrap()).lines();
        stream.write_all(HELLO).unwrap();
        assert!(lines.next().unwrap().unwrap().contains("\"ok\""));
        stream.write_all(b"{\"v\":1,\"type\":\"list_models\",\"id\":\"same\"}\n").unwrap();
        assert!(lines.next().unwrap().unwrap().contains("\"same\""), "the same process goes on");

        let request = b"{\"v\":1,\"type\":\"list_models\",\"id\":\"child\"}\n";
        // SAFETY: the child makes only calls that are safe after a fork of a process with threads.
        unsafe {
            let child = libc::fork();
            assert!(child >= 0);
            if child == 0 {
                libc::write(stream.as_raw_fd(), request.as_ptr().cast(), request.len());
                libc::_exit(0);
            }
            libc::waitpid(child, std::ptr::null_mut(), 0);
        }
        assert!(lines.next().is_none(), "no reply, and the host closes");
        eventually("the host forgets the connection", || host.connected_apps().is_empty());
    }

    /// Needs the right to make namespaces, so it runs in a Linux container
    /// with this right, and passes without doing anything elsewhere.
    #[cfg(target_os = "linux")]
    mod namespaces {
        use super::*;
        use std::process::Command;

        fn may_unshare() -> bool {
            Command::new("unshare").args(["--mount", "--pid", "--fork", "true"]).status().is_ok_and(|status| status.success())
        }

        fn example_copy(folder: &Path) -> PathBuf {
            let copy = folder.join("sayso-ext");
            std::fs::copy(example(), &copy).unwrap();
            copy
        }

        #[test]
        fn a_bind_mount_over_an_approved_path_is_not_the_approved_program() {
            if !may_unshare() {
                eprintln!("skipped: no right to make namespaces");
                return;
            }
            let setup = setup();
            let folder = tempfile::tempdir().unwrap();
            let own = example_copy(folder.path());
            // The path has an approved program of another file.
            let approved = folder.path().join("approved");
            std::fs::copy("/bin/true", &approved).unwrap();
            write_app(&setup.apps, &approved, &[]);
            let _host = start_with(&setup, FakeServices::new(None));
            let script = format!("mount --bind '{}' '{}' && exec '{}' hello", own.display(), approved.display(), approved.display());
            let output = Command::new("unshare").args(["--mount", "sh", "-c", &script]).env("SAYSO_HOST_SOCKET", &setup.socket).output().unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(!output.status.success() && stderr.contains("did not approve"), "{output:?}");
        }

        #[test]
        fn a_process_of_another_pid_namespace_has_no_identity() {
            if !may_unshare() {
                eprintln!("skipped: no right to make namespaces");
                return;
            }
            let setup = setup();
            let folder = tempfile::tempdir().unwrap();
            let own = example_copy(folder.path());
            write_app(&setup.apps, &own, &[]);
            let _host = start_with(&setup, FakeServices::new(None));
            let run = |command: &mut Command| command.arg(&own).arg("hello").env("SAYSO_HOST_SOCKET", &setup.socket).output().unwrap();
            assert!(run(&mut Command::new("env")).status.success(), "the same program, in the namespace of the host");
            let output = run(Command::new("unshare").args(["--pid", "--fork"]));
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(!output.status.success() && stderr.contains("did not approve"), "{output:?}");
        }
    }
}
