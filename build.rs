use std::process::{Command, ExitStatus, Output};

#[cfg(not(target_os = "windows"))]
use std::os::unix::process::ExitStatusExt;

#[cfg(target_os = "windows")]
use std::os::windows::process::ExitStatusExt;

fn main() {
	println!("cargo:rerun-if-changed=src/");
	// Container and release builds have no .git in their context, so CI passes the commit in GIT_HASH.
	println!("cargo:rerun-if-env-changed=GIT_HASH");
	let from_env = std::env::var("GIT_HASH").unwrap_or_default();
	let git_hash = if !from_env.trim().is_empty() { from_env.trim().to_string() } else { git_rev_parse() };
	println!("cargo:rustc-env=GIT_HASH={git_hash}");
}

fn git_rev_parse() -> String {
	let output = String::from_utf8(
		Command::new("git")
			.args(["rev-parse", "HEAD"])
			.output()
			.unwrap_or(Output {
				stdout: vec![],
				stderr: vec![],
				status: ExitStatus::from_raw(0),
			})
			.stdout,
	)
	.unwrap_or_default();
	if output.trim().is_empty() {
		"dev".into()
	} else {
		output.trim().to_string()
	}
}
