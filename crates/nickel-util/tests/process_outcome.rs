#![allow(clippy::unwrap_used)]
use nickel_util::process::{capture, capture_outcome, Completion, Stream};
use std::{
    process::Command,
    time::{Duration, Instant},
};

fn shell(script: &str) -> Command {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", script]);
    command
}

#[test]
fn failed_outcome_retains_both_streams_but_debug_does_not() {
    let output = capture_outcome(
        shell("printf private-out; printf private-err >&2; exit 7"),
        Duration::from_secs(2),
        1024,
    )
    .unwrap();
    assert!(matches!(output.completion, Completion::Exited(s) if s.code()==Some(7)));
    assert_eq!(output.stdout, b"private-out");
    assert_eq!(output.stderr, b"private-err");
    assert_eq!(
        format!("{output:?}"),
        format!(
            "Captured {{ completion: {:?}, stdout_bytes: 11, stderr_bytes: 11 }}",
            output.completion
        )
    );
    let old = capture(
        shell("printf private-err >&2; exit 7"),
        Duration::from_secs(2),
        1024,
    )
    .unwrap_err();
    assert!(
        matches!(old, nickel_util::error::Error::ProcessExit {status, stderr, ..} if status.code()==Some(7) && stderr=="private-err")
    );
}

#[test]
fn timeout_and_both_stream_limits_keep_bounded_partial_output() {
    let start = Instant::now();
    let output = capture_outcome(
        shell("printf partial; sleep 10 & wait"),
        Duration::from_millis(100),
        1024,
    )
    .unwrap();
    assert!(matches!(output.completion, Completion::TimedOut));
    assert_eq!(output.stdout, b"partial");
    assert!(start.elapsed() < Duration::from_secs(3));
    for (script, expected) in [
        (
            "printf retained-prefix; while :; do printf 12345678; done",
            Stream::Stdout,
        ),
        (
            "printf retained-prefix >&2; while :; do printf 12345678 >&2; done",
            Stream::Stderr,
        ),
    ] {
        let output = capture_outcome(shell(script), Duration::from_secs(2), 8192).unwrap();
        assert!(matches!(output.completion, Completion::OutputLimit {stream} if stream==expected));
        assert!(output.stdout.len() <= 8192 && output.stderr.len() <= 8192);
        let retained = if expected == Stream::Stdout {
            &output.stdout
        } else {
            &output.stderr
        };
        assert!(retained.starts_with(b"retained-prefix"));
    }
}

#[test]
fn exited_parent_does_not_wait_for_descendants_holding_pipes() {
    let start = Instant::now();
    let output = capture_outcome(
        shell("sleep 10 & printf done"),
        Duration::from_secs(2),
        1024,
    )
    .unwrap();
    assert!(matches!(output.completion,Completion::Exited(s) if s.success()));
    assert_eq!(output.stdout, b"done");
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn timeout_reaps_direct_child_and_stops_descendant() {
    let output = capture_outcome(
        shell("sleep 10 & child=$!; printf '%s %s' \"$$\" \"$child\"; wait"),
        Duration::from_millis(500),
        1024,
    )
    .unwrap();
    assert!(matches!(output.completion, Completion::TimedOut));
    let text = String::from_utf8(output.stdout).unwrap();
    let ids: Vec<i32> = text
        .split_whitespace()
        .map(|s| s.parse().unwrap())
        .collect();
    assert_eq!(ids.len(), 2);
    let mut status = 0;
    // SAFETY: a valid output pointer is supplied; PID came from the owned child.
    assert_eq!(
        unsafe { libc::waitpid(ids[0], &mut status, libc::WNOHANG) },
        -1
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD)
    );
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let result = std::fs::read_to_string(format!("/proc/{}/stat", ids[1]));
        if matches!(&result, Err(error) if error.kind() == std::io::ErrorKind::NotFound) {
            break;
        }
        let stat = result.unwrap();
        if stat.rsplit_once(") ").unwrap().1.split_whitespace().next() == Some("Z") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "descendant did not stop after group termination"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
