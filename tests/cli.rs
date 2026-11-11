use std::fs;
use std::process::Command;

use tempfile::TempDir;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_dulens")
}

fn command() -> Command {
    let mut command = Command::new(bin());
    command.env("COLUMNS", "100");
    command
}

#[test]
fn prints_a_tree_and_exact_sizes() {
    let temp = TempDir::new().unwrap();
    fs::create_dir(temp.path().join("sub")).unwrap();
    fs::write(temp.path().join("sub/data.bin"), vec![b'x'; 1234]).unwrap();

    let output = command()
        .args([
            "--color",
            "never",
            "--apparent-size",
            "--bytes",
            "--all",
            "-d",
            "2",
        ])
        .arg(temp.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("dulens"));
    assert!(stdout.contains("sub/"));
    assert!(stdout.contains("data.bin"));
    assert!(stdout.contains("1234 B"));
    assert!(!stdout.contains('\x1b'));
}

#[test]
fn excludes_directories() {
    let temp = TempDir::new().unwrap();
    fs::create_dir(temp.path().join("keep")).unwrap();
    fs::create_dir(temp.path().join("skip")).unwrap();
    fs::write(temp.path().join("keep/a"), vec![b'a'; 10]).unwrap();
    fs::write(temp.path().join("skip/b"), vec![b'b'; 10]).unwrap();

    let output = command()
        .args(["--color", "never", "-a", "-E", "skip"])
        .arg(temp.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("keep/"));
    assert!(!stdout.contains("skip"));
}

#[test]
fn reports_missing_paths_with_a_nonzero_exit_status() {
    let temp = TempDir::new().unwrap();
    let missing = temp.path().join("does-not-exist");

    let output = command().arg(missing).output().unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("warning"));
    assert!(stderr.contains("no readable paths"));
}

#[test]
fn cleanup_mode_requires_a_terminal() {
    let output = Command::new(bin())
        .arg("--cleanup")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .expect("run dulens --clean");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cleanup mode requires a terminal"),
        "stderr: {stderr}"
    );
}

#[test]
fn interactive_mode_requires_a_terminal() {
    let temp = TempDir::new().unwrap();
    let output = command().args(["-i"]).arg(temp.path()).output().unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("requires a terminal"));
}

#[test]
fn rejects_invalid_exclude_globs() {
    let temp = TempDir::new().unwrap();
    let output = command()
        .args(["-E", "["])
        .arg(temp.path())
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("invalid exclude pattern"));
}

#[cfg(unix)]
#[test]
fn hardlinks_are_counted_once_across_multiple_roots() {
    let temp = TempDir::new().unwrap();
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    fs::create_dir(&first).unwrap();
    fs::create_dir(&second).unwrap();
    fs::write(first.join("data"), vec![b'x'; 1000]).unwrap();
    fs::hard_link(first.join("data"), second.join("link")).unwrap();

    let output = command()
        .args(["--color", "never", "--apparent-size", "--bytes", "-d", "0"])
        .arg(&first)
        .arg(&second)
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let total = stdout.split_whitespace().nth(1).unwrap();
    assert_eq!(total, "1000");
}

#[test]
fn supports_si_units() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("data"), vec![b'x'; 1000]).unwrap();

    let output = command()
        .args(["--color", "never", "--apparent-size", "--si", "-d", "0"])
        .arg(temp.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("1.00 kB"));
}

#[test]
fn quiet_hides_warnings_but_keeps_failure_status() {
    let temp = TempDir::new().unwrap();
    let missing = temp.path().join("missing");

    let output = command()
        .args(["--quiet", "--color", "never"])
        .arg(missing)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("no readable paths"));
    assert!(!stderr.contains("warning"));
}

#[test]
fn plain_mode_has_no_decoration() {
    let temp = TempDir::new().unwrap();
    fs::create_dir(temp.path().join("sub")).unwrap();
    fs::write(temp.path().join("sub/data.bin"), vec![b'x'; 1234]).unwrap();

    let output = command()
        .args([
            "--plain",
            "--color",
            "always",
            "--apparent-size",
            "--bytes",
            "--all",
            "-d",
            "2",
        ])
        .arg(temp.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains("dulens"));
    assert!(!stdout.contains('%'));
    assert!(!stdout.contains('█'));
    assert!(!stdout.contains('░'));
    assert!(!stdout.contains('│'));
    assert!(!stdout.contains('├'));
    assert!(!stdout.contains('\x1b'));
    assert!(stdout.contains("sub/"));
    assert!(stdout.contains("data.bin"));
    assert!(stdout.contains("1234 B"));
}
