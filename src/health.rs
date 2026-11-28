//! `GET /healthz`: a cheap liveness and upstream-readiness signal for container healthchecks.
//!
//! It reads in-memory OAuth state only and never calls Reddit, so a probe every few seconds
//! costs nothing against the rate limit.

use crate::client::{OAUTH_RATELIMIT_REMAINING, OAUTH_TOKEN_EXPIRES_AT};
use hyper::{Body, Response};
use serde::Serialize;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct HealthReport {
	/// `"ok"` when requests to Reddit can succeed, `"degraded"` when the OAuth token is missing or expired.
	pub status: &'static str,
	pub oauth_ready: bool,
	/// Requests left in the current Reddit rate-limit window. Informational: it refills on its own.
	pub ratelimit_remaining: u16,
}

impl HealthReport {
	/// `token_expires_at` is a Unix timestamp in seconds; `0` means no token was ever installed.
	pub fn assess(now_unix: u64, token_expires_at: u64, ratelimit_remaining: u16) -> Self {
		let oauth_ready = token_expires_at > now_unix;
		Self {
			status: if oauth_ready { "ok" } else { "degraded" },
			oauth_ready,
			ratelimit_remaining,
		}
	}

	pub fn is_healthy(&self) -> bool {
		self.oauth_ready
	}
}

/// Answers `200` while the OAuth token is valid and `503` otherwise, with the report as JSON.
pub async fn healthz() -> Result<Response<Body>, String> {
	let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs());
	let report = HealthReport::assess(now, OAUTH_TOKEN_EXPIRES_AT.load(Ordering::SeqCst), OAUTH_RATELIMIT_REMAINING.load(Ordering::SeqCst));
	let status = if report.is_healthy() { 200 } else { 503 };
	let body = serde_json::to_string(&report).map_err(|e| e.to_string())?;
	Response::builder()
		.status(status)
		.header("content-type", "application/json")
		.header("cache-control", "no-store")
		.body(Body::from(body))
		.map_err(|e| e.to_string())
}

/// Where a probe should connect for a server that listens on `listen`: a wildcard address
/// is not connectable, so it becomes loopback (`::` also tries `::1`, in case IPv4-mapped
/// IPv6 is off).
pub fn probe_addresses(listen: SocketAddr) -> Vec<SocketAddr> {
	match listen.ip() {
		IpAddr::V4(ip) if ip.is_unspecified() => vec![SocketAddr::new(Ipv4Addr::LOCALHOST.into(), listen.port())],
		IpAddr::V6(ip) if ip.is_unspecified() => vec![
			SocketAddr::new(Ipv4Addr::LOCALHOST.into(), listen.port()),
			SocketAddr::new(Ipv6Addr::LOCALHOST.into(), listen.port()),
		],
		_ => vec![listen],
	}
}

/// Total budget for `--healthcheck` across all candidate addresses (e.g. IPv4 + IPv6 for `[::]`).
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Asks the server that listens on `listen` for `/healthz`. `Ok` means it answered `200`;
/// the error says why it did not, for the `--healthcheck` message.
pub async fn probe(listen: SocketAddr) -> Result<(), String> {
	probe_addresses_within(&probe_addresses(listen), PROBE_TIMEOUT).await
}

async fn probe_addresses_within(addresses: &[SocketAddr], budget: Duration) -> Result<(), String> {
	let deadline = tokio::time::Instant::now() + budget;
	let mut last_error = String::new();
	for &address in addresses {
		let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
		if remaining.is_zero() {
			return Err(format!("timed out after {budget:?} ({last_error})"));
		}
		let uri: hyper::Uri = format!("http://{address}/healthz").parse().map_err(|e| format!("bad probe address {address}: {e}"))?;
		last_error = match tokio::time::timeout(remaining, hyper::Client::new().get(uri)).await {
			Ok(Ok(response)) if response.status() == 200 => return Ok(()),
			Ok(Ok(response)) => format!("{address} answered {}", response.status()),
			Ok(Err(e)) => format!("cannot reach {address}: {e}"),
			Err(_) => return Err(format!("{address} did not answer within {budget:?}")),
		};
	}
	Err(last_error)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn healthz_and_shutdown_probe_shares_one_deadline_across_addresses() {
		let first = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let second = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let addresses = [first.local_addr().unwrap(), second.local_addr().unwrap()];
		let silent = tokio::spawn(async move {
			let _a = first.accept().await;
			let _b = second.accept().await;
			std::future::pending::<()>().await;
		});

		let started = tokio::time::Instant::now();
		let result = probe_addresses_within(&addresses, Duration::from_millis(200)).await;
		let elapsed = started.elapsed();
		silent.abort();

		assert!(result.is_err());
		assert!(elapsed < Duration::from_millis(380), "shared budget should not double across two addresses: {elapsed:?}");
	}

	#[test]
	fn healthz_and_shutdown_probe_addresses_turn_wildcards_into_loopback() {
		let addresses = |listen: &str| probe_addresses(listen.parse().unwrap()).iter().map(ToString::to_string).collect::<Vec<_>>();
		assert_eq!(addresses("0.0.0.0:8080"), ["127.0.0.1:8080"]);
		assert_eq!(addresses("[::]:8080"), ["127.0.0.1:8080", "[::1]:8080"]);
		assert_eq!(addresses("192.168.1.5:9000"), ["192.168.1.5:9000"]);
		assert_eq!(addresses("[::1]:8080"), ["[::1]:8080"]);
	}

	#[test]
	fn healthz_and_shutdown_report_ok_with_a_live_token() {
		let report = HealthReport::assess(1_000, 5_000, 87);
		assert_eq!(
			report,
			HealthReport {
				status: "ok",
				oauth_ready: true,
				ratelimit_remaining: 87
			}
		);
		assert!(report.is_healthy());
	}

	#[test]
	fn healthz_and_shutdown_report_degraded_when_the_token_expired_or_missing() {
		for (now, expires_at) in [(5_000, 1_000), (5_000, 5_000), (5_000, 0)] {
			let report = HealthReport::assess(now, expires_at, 12);
			assert_eq!(report.status, "degraded", "now {now}, expires {expires_at}");
			assert!(!report.oauth_ready && !report.is_healthy());
			assert_eq!(report.ratelimit_remaining, 12, "the rate limit is reported even when degraded");
		}
	}
}
