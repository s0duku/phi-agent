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
    let command = serde_json::json!({"Shell": {"command": command}}).to_string();

    let launched = Command::new(PHI)
        .args(["headlessterm", "launch-local", &handle, "5000", &command])
        .output()
        .expect("container launcher should execute");
    assert!(
        launched.status.success(),
        "container launcher failed: {}",
        String::from_utf8_lossy(&launched.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&launched.stdout)
            .expect("launcher stdout should be a JSON report"),
        serde_json::json!({"status": "ready", "handle": handle})
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    let ready = loop {
        let output = Command::new(PHI)
            .args(["headlessterm", "access", "--wait-ms", "0", &handle])
            .output()
            .expect("container access should execute");
        let result = serde_json::from_slice::<phi::headlessterm::JobAccessResult>(&output.stdout);
        if output.status.success()
            && matches!(
                result.as_ref(),
                Ok(phi::headlessterm::JobAccessResult::Interacted(info))
                    if info.is_running()
            )
        {
            break result.unwrap();
        }
        assert!(
            output.status.success()
                && matches!(
                    result.as_ref(),
                    Ok(phi::headlessterm::JobAccessResult::Interacted(info))
                        if matches!(info.status(), phi::headlessterm::JobStatus::NoExist)
                ),
            "unexpected container status while waiting for readiness: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        assert!(
            Instant::now() < deadline,
            "detached container did not become ready: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    let phi::headlessterm::JobAccessResult::Interacted(ready) = ready else {
        panic!("access should return an interacted result");
    };
    assert!(ready.is_running());

    let closed = Command::new(PHI)
        .args(["headlessterm", "close", &handle])
        .output()
        .expect("container close should execute");
    assert!(
        closed.status.success(),
        "container close failed: {}",
        String::from_utf8_lossy(&closed.stderr)
    );
    serde_json::from_slice::<phi::headlessterm::JobInfo>(&closed.stdout)
        .expect("close stdout should be the API JobInfo value");
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
