//! A client that drives the built `sayso-cleanup` binary over its pipes.
#![allow(dead_code)]

use sayso_core::cleanup::protocol::{Reply, ReplyLine, Request, RequestLine};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

pub struct Session {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next: u32,
}

impl Session {
    pub fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_sayso-cleanup"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("cannot start sayso-cleanup");
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Session {
            child,
            stdin,
            stdout,
            next: 1,
        }
    }

    /// Send a raw line and read one reply line.
    pub fn raw(&mut self, line: &str) -> ReplyLine {
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(line.as_bytes()).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
        let mut reply = String::new();
        self.stdout.read_line(&mut reply).unwrap();
        serde_json::from_str(&reply).unwrap_or_else(|e| panic!("bad reply {reply:?}: {e}"))
    }

    /// Send a request and read its reply, which must carry the same id.
    pub fn call(&mut self, request: Request) -> Reply {
        let id = self.next.to_string();
        self.next += 1;
        let line = serde_json::to_string(&RequestLine::new(id.clone(), request)).unwrap();
        let reply = self.raw(&line);
        assert_eq!(reply.id, id);
        reply.reply
    }

    /// Close stdin and wait for the exit.
    pub fn finish(mut self) -> std::process::ExitStatus {
        drop(self.stdin.take());
        let mut rest = String::new();
        self.stdout.read_line(&mut rest).unwrap();
        assert_eq!(rest, "", "the runtime wrote after the last reply");
        self.child.wait().unwrap()
    }
}
