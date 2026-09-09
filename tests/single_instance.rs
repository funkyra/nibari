use std::fs::{self, File};
use std::process::Command;

#[test]
fn rejects_duplicate_start_and_allows_restart_after_unlock() {
    let runtime = std::env::temp_dir().join(format!("nibari-instance-test-{}", std::process::id()));
    fs::create_dir(&runtime).unwrap();
    let lock = File::create(runtime.join("nibari.lock")).unwrap();
    lock.try_lock().unwrap();

    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_nibari"))
            .args(args)
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("XDG_CONFIG_HOME", &runtime)
            .env("WAYLAND_DISPLAY", "missing-wayland-socket")
            .env_remove("WAYLAND_SOCKET")
            .output()
            .unwrap()
    };

    let duplicate = run(&[]);
    assert_eq!(duplicate.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&duplicate.stderr).contains("nibari is already running"),
        "unexpected error: {}",
        String::from_utf8_lossy(&duplicate.stderr)
    );
    for arg in ["--help", "--print-default-config"] {
        assert!(run(&[arg]).status.success());
    }

    drop(lock);
    // The lock file stays behind, but an unlocked file must not prevent startup.
    for _ in 0..2 {
        let restarted = run(&[]);
        assert!(
            String::from_utf8_lossy(&restarted.stderr).contains("failed to connect to Wayland"),
            "unexpected error: {}",
            String::from_utf8_lossy(&restarted.stderr)
        );
    }
    fs::remove_dir_all(&runtime).unwrap();
}
