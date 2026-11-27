//! Runs the built `redlib --healthcheck` against a stub HTTP server on loopback. No Reddit access is needed:
//! the flag must answer before config loading and OAuth.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Output, Stdio};

/// Serves one canned response to the first connection, then exits.
fn stub_server(status_line: &'static str) -> (u16, std::thread::JoinHandle<String>) {
	let listener = TcpListener::bind("127.0.0.1:0").unwrap();
	let port = listener.local_addr().unwrap().port();
	let handle = std::thread::spawn(move || {
		let (mut socket, _) = listener.accept().unwrap();
		let mut request = [0; 1024];
		let read = socket.read(&mut request).unwrap();
		write!(socket, "HTTP/1.1 {status_line}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
		String::from_utf8_lossy(&request[..read]).lines().next().unwrap_or_default().to_string()
	});
	(port, handle)
}

fn healthcheck(port: u16) -> Output {
	let dir = std::env::temp_dir().join(format!("redlib-healthcheck-{}-{port}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let output = Command::new(env!("CARGO_BIN_EXE_redlib"))
		.args(["--healthcheck", "-a", "127.0.0.1", "-p", &port.to_string()])
		.current_dir(&dir)
		.stdin(Stdio::null())
		.output()
		.unwrap();
	_ = std::fs::remove_dir_all(&dir);
	output
}

#[test]
fn healthz_and_shutdown_healthcheck_exits_0_for_200() {
	let (port, request_line) = stub_server("200 OK");
	let output = healthcheck(port);
	assert_eq!(output.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&output.stderr));
	assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "ok");
	assert_eq!(request_line.join().unwrap(), "GET /healthz HTTP/1.1");
}

#[test]
fn healthz_and_shutdown_healthcheck_exits_1_for_503() {
	let (port, _) = stub_server("503 Service Unavailable");
	let output = healthcheck(port);
	assert_eq!(output.status.code(), Some(1));
	assert!(
		String::from_utf8_lossy(&output.stderr).contains("503"),
		"stderr: {}",
		String::from_utf8_lossy(&output.stderr)
	);
	assert!(output.stdout.is_empty(), "failures go to stderr only");
}

#[test]
fn healthz_and_shutdown_healthcheck_exits_1_when_nothing_listens() {
	// Bind and release a port so nothing is listening on it.
	let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
	let output = healthcheck(port);
	assert_eq!(output.status.code(), Some(1));
	assert!(!output.stderr.is_empty());
}
