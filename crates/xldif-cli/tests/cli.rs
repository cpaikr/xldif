//! The CLI as an external process: the process contract.

use std::process::Command;

use serde_json::Value;

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

fn xldif<const N: usize>(args: [&str; N]) -> Run {
    let out = Command::new(env!("CARGO_BIN_EXE_xldif")).args(args).output().unwrap();
    Run {
        code: out.status.code().unwrap(),
        stdout: String::from_utf8(out.stdout).unwrap(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

fn envelope(run: &Run) -> Value {
    assert!(run.stdout.ends_with('\n') && run.stdout.matches('\n').count() == 1, "one line: {:?}", run.stdout);
    serde_json::from_str(&run.stdout).unwrap()
}

#[test]
fn bare_invocation_prints_root_help() {
    let bare = xldif([]);
    let help = xldif(["--help"]);
    assert_eq!((bare.code, help.code), (0, 0));
    assert_eq!(bare.stdout, help.stdout);
    assert!(bare.stdout.starts_with("Compare Excel workbooks"));
    assert!(bare.stderr.is_empty());
}

#[test]
fn version_is_plain_text() {
    let v = xldif(["--version"]);
    assert_eq!(v.code, 0);
    assert_eq!(v.stdout, format!("xldif {}\n", env!("CARGO_PKG_VERSION")));
    assert!(v.stderr.is_empty());
}

#[test]
fn invalid_invocation_is_a_json_failure_with_exit_2() {
    let run = xldif(["no-such-command"]);
    assert_eq!(run.code, 2);
    assert!(run.stderr.is_empty());
    let env = envelope(&run);
    assert_eq!(env["ok"], false);
    assert_eq!(env["command"], "no-such-command");
    assert_eq!(env["result"], Value::Null);
    assert_eq!(env["error"]["code"], "invalid_invocation");
    assert!(env["error"]["hint"].as_str().unwrap().contains("xldif --help"));
    assert_eq!(env["metadata"]["tool"], "xldif");
    assert_eq!(env["metadata"]["contract"], "1");
    assert_eq!(env["metadata"]["version"], env!("CARGO_PKG_VERSION"));
}

#[test]
fn pretty_indents_failures() {
    let run = xldif(["--pretty", "--no-such-flag"]);
    assert_eq!(run.code, 2);
    assert!(run.stdout.starts_with("{\n"));
    let env: Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(env["command"], Value::Null);
    assert_eq!(env["error"]["code"], "invalid_invocation");
}
