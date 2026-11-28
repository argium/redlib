//! Runs the built `redlib` binary against a malformed `redlib.toml`. Nothing here touches the network:
//! the binary must stop before it reaches the rate limit check.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// A unique working directory holding a malformed `redlib.toml`, removed on drop.
struct BrokenConfigDir(PathBuf);

impl BrokenConfigDir {
	fn new(test_name: &str) -> Self {
		let dir = std::env::temp_dir().join(format!("redlib-{test_name}-{}", std::process::id()));
		std::fs::create_dir_all(&dir).unwrap();
		std::fs::write(dir.join("redlib.toml"), "REDLIB_BANNER = [unterminated\n").unwrap();
		Self(dir)
	}

	fn path(&self) -> &Path {
		&self.0
	}
}

impl Drop for BrokenConfigDir {
	fn drop(&mut self) {
		_ = std::fs::remove_dir_all(&self.0);
	}
}

/// Runs the binary and fails the test if it is still running after `limit`,
/// which is how an unwanted network call would show up.
fn run_redlib(dir: &Path, args: &[&str], limit: Duration) -> Output {
	let mut child = Command::new(env!("CARGO_BIN_EXE_redlib"))
		.args(args)
		.current_dir(dir)
		.stdin(Stdio::null())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.spawn()
		.unwrap();
	let started = Instant::now();
	while child.try_wait().unwrap().is_none() {
		if started.elapsed() > limit {
			child.kill().unwrap();
			panic!("redlib was still running after {limit:?}; it should have exited before any network use");
		}
		std::thread::sleep(Duration::from_millis(20));
	}
	child.wait_with_output().unwrap()
}

#[test]
fn startup_config_malformed_file_stops_startup_with_exit_1_and_names_the_file() {
	let dir = BrokenConfigDir::new("malformed-start");
	let output = run_redlib(dir.path(), &[], Duration::from_secs(20));
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert_eq!(output.status.code(), Some(1), "stderr: {stderr}");
	assert!(stderr.contains("redlib.toml"), "diagnostic should name the file: {stderr}");
}

#[test]
fn startup_config_version_works_with_a_malformed_file() {
	let dir = BrokenConfigDir::new("malformed-version");
	let output = run_redlib(dir.path(), &["--version"], Duration::from_secs(20));
	assert_eq!(output.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&output.stderr));
	assert!(String::from_utf8_lossy(&output.stdout).contains(env!("CARGO_PKG_VERSION")));
}
