use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PHI: &str = env!("CARGO_BIN_EXE_phi");

#[test]
fn detached_container_survives_its_launcher() {
    let handle = unique_handle();
    let command = if cfg!(windows) {
        "Start-Sleep -Seconds 30"
    } else {
        "sleep 30"
    };

    let launched = Command::new(PHI)
        .args(["headlessterm", "launch-local", &handle, "5000", command])
        .output()
        .expect("container launcher should execute");
    assert!(
        launched.status.success(),
        "container launcher failed: {}",
        String::from_utf8_lossy(&launched.stderr)
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    let ready = loop {
        let output = Command::new(PHI)
            .args(["headlessterm", "write", "--wait-ms", "0", &handle])
            .output()
            .expect("container access should execute");
        let stderr = String::from_utf8_lossy(&output.stderr);
        if output.status.success() && stderr.contains("status=running") {
            break output;
        }
        assert!(
            output.status.success() && stderr.contains("status=not-found"),
            "unexpected container status while waiting for readiness: {stderr}"
        );
        assert!(
            Instant::now() < deadline,
            "detached container did not become ready: {stderr}"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(
        String::from_utf8_lossy(&ready.stderr).contains("status=running"),
        "detached container reported an unexpected ready status: {}",
        String::from_utf8_lossy(&ready.stderr)
    );

    let closed = Command::new(PHI)
        .args(["headlessterm", "close", &handle])
        .output()
        .expect("container close should execute");
    assert!(
        closed.status.success(),
        "container close failed: {}",
        String::from_utf8_lossy(&closed.stderr)
    );
}

fn unique_handle() -> String {
    let mut seed = u64::from(std::process::id())
        ^ SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
    let mut letters = [b'a'; 8];
    for letter in &mut letters {
        *letter = b'a' + (seed % 26) as u8;
        seed /= 26;
    }
    let letters = std::str::from_utf8(&letters).expect("generated handle should be ASCII");
    format!("{}-{}", &letters[..4], &letters[4..])
}
