use std::{
    io::Write,
    process::{Command, Stdio},
};

fn tis() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tis"))
}

#[test]
fn runs_files_inline_code_and_stdin() {
    let inline = tis().args(["-c", "print('東京')"]).output().unwrap();
    assert!(inline.status.success());
    assert_eq!(String::from_utf8(inline.stdout).unwrap(), "東京\n");

    let file = tis().arg("tests/fixtures/language.py").output().unwrap();
    assert!(
        file.status.success(),
        "{}",
        String::from_utf8_lossy(&file.stderr)
    );
    assert!(String::from_utf8_lossy(&file.stdout).contains("language core ok"));

    let mut child = tis()
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"print(42)").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"42\n");
}

#[test]
fn errors_fail_and_keep_prior_output() {
    let output = tis()
        .args(["-c", "print(42)\nprint(missing)"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(output.stdout, b"42\n");
    assert!(!output.stderr.is_empty());

    for args in [
        vec!["--unknown"],
        vec!["-c"],
        vec!["--tier", "unknown"],
        vec![],
    ] {
        assert!(!tis().args(args).output().unwrap().status.success());
    }
}

#[test]
fn help_version_and_tier_options() {
    for option in ["--help", "--version"] {
        assert!(tis().arg(option).output().unwrap().status.success());
    }
    for tier in ["bytecode", "lir", "native", "copy-patch"] {
        let output = tis()
            .args(["--tier", tier, "--stats", "-c", "print(1+2)"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"3\n");
        assert!(!output.stderr.is_empty());
    }
}
