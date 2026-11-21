#![allow(clippy::unwrap_used)]
#![cfg(feature = "cli")]
use std::process::Command;
#[test]
fn exposes_offline_and_controller_commands_and_rejects_missing_inputs() {
    let binary = env!("CARGO_BIN_EXE_nickel-plane");
    let help = Command::new(binary).arg("--help").output().unwrap();
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    for verb in [
        "check",
        "render",
        "lock",
        "plan",
        "apply",
        "status",
        "reconcile",
        "serve",
        "remove",
    ] {
        assert!(help.contains(verb), "missing {verb}");
    }
    assert!(!Command::new(binary)
        .arg("apply")
        .output()
        .unwrap()
        .status
        .success());
}
