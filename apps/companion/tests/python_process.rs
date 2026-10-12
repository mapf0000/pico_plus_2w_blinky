//! Exercise the production executable, not a VM inside the test harness.
use std::{
    io::Write,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
fn output(mut child: std::process::Child, timeout: Duration) -> std::process::Output {
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Python child acceptance exceeded its deadline");
        }
        thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn production_python_supervisor_containment_and_effect_lifecycle() {
    let child = Command::new(env!("CARGO_BIN_EXE_pico-companion"))
        .arg("--python-self-test")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let output = output(child, Duration::from_secs(40));
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    for evidence in [
        "Python generator/effects passed",
        "button wait passed",
        "disconnect/throw/reconnect retained state",
        "Python step exceeded 500 ms",
        "MemoryError",
        "ImportError",
        "Stop during non-yielding startup",
        "Stop cancelled only its own",
        "Repeated allocation pressure recovered",
    ] {
        assert!(
            text.contains(evidence),
            "Missing acceptance evidence: {evidence}"
        );
    }
}
#[test]
fn worker_rejects_excess_lengths_unknown_versions_and_bad_order() {
    for payload in [
        vec![255; 4],
        framed(br#"{"version":2,"run":1,"step":1,"input":{"kind":"start","source":"pass"}}"#),
        framed(br#"{"version":1,"run":1,"step":2,"input":{"kind":"resume","value":null}}"#),
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_pico-companion"))
            .arg("--python-worker")
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&payload).unwrap();
        let output = output(child, Duration::from_secs(3));
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
}
fn framed(payload: &[u8]) -> Vec<u8> {
    let mut bytes = (payload.len() as u32).to_le_bytes().to_vec();
    bytes.extend_from_slice(payload);
    bytes
}
