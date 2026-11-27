//! Coalesces concurrent identical work, so a burst of requests for one Reddit path spends
//! one rate-limit slot instead of one each.

use std::collections::HashMap;
use std::future::Future;
use std::hash::Hash;
use std::sync::Mutex;
use tokio::sync::broadcast;

/// Runs one piece of work per key at a time. The first caller for a key does the work;
/// callers that arrive while it runs wait for that result and receive a clone of it, `Err`s
/// included. An entry exists only while its work is running, so nothing accumulates.
pub struct SingleFlight<K, V> {
	in_flight: Mutex<HashMap<K, broadcast::Sender<V>>>,
}

impl<K: Hash + Eq + Clone, V: Clone> SingleFlight<K, V> {
	pub fn new() -> Self {
		Self {
			in_flight: Mutex::new(HashMap::new()),
		}
	}

	/// Returns the result of `work`, or of the identical work another caller already has running
	/// for `key`. If that caller is cancelled before it finishes (a visitor disconnecting),
	/// this caller takes over and runs its own `work`.
	pub async fn run(&self, key: K, work: impl Future<Output = V>) -> V {
		let mut work = Some(work);
		loop {
			let role = {
				let mut in_flight = self.lock();
				match in_flight.get(&key) {
					Some(leader) => Role::Waiter(leader.subscribe()),
					None => {
						let (sender, _) = broadcast::channel(1);
						in_flight.insert(key.clone(), sender.clone());
						Role::Leader(sender)
					}
				}
			};

			match role {
				Role::Waiter(mut result) => {
					// `Err` means the leader was dropped before it finished: try again.
					if let Ok(value) = result.recv().await {
						return value;
					}
				}
				Role::Leader(sender) => {
					let entry = Entry { flights: self, key: &key };
					let value = work.take().expect("a leader returns, so it runs the work once").await;
					// Remove the entry first, so a caller arriving later starts fresh work
					// instead of subscribing to a channel that already sent.
					drop(entry);
					let _ = sender.send(value.clone());
					return value;
				}
			}
		}
	}

	fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<K, broadcast::Sender<V>>> {
		self.in_flight.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
	}
}

enum Role<V> {
	Leader(broadcast::Sender<V>),
	Waiter(broadcast::Receiver<V>),
}

/// Removes a key's in-flight entry when dropped, including when the leader is cancelled.
struct Entry<'a, K: Hash + Eq + Clone, V: Clone> {
	flights: &'a SingleFlight<K, V>,
	key: &'a K,
}

impl<K: Hash + Eq + Clone, V: Clone> Drop for Entry<'_, K, V> {
	fn drop(&mut self) {
		self.flights.lock().remove(self.key);
	}
}

impl<K: Hash + Eq + Clone, V: Clone> Default for SingleFlight<K, V> {
	fn default() -> Self {
		Self::new()
	}
}

#[cfg(test)]
impl<K: Hash + Eq + Clone, V: Clone> SingleFlight<K, V> {
	fn entries(&self) -> usize {
		self.lock().len()
	}

	fn waiters(&self, key: &K) -> usize {
		self.lock().get(key).map_or(0, broadcast::Sender::receiver_count)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::atomic::{AtomicUsize, Ordering};
	use std::sync::Arc;
	use tokio::sync::Notify;

	type Flights = Arc<SingleFlight<String, Result<String, String>>>;

	/// Starts `count` callers for one key whose work blocks on `gate`, waits until all but the
	/// leader are queued behind it, then opens the gate.
	async fn run_burst(flights: &Flights, count: usize, outcome: Result<String, String>) -> (Vec<Result<String, String>>, usize) {
		let fetches = Arc::new(AtomicUsize::new(0));
		let gate = Arc::new(Notify::new());
		let key = "/r/rust/hot.json".to_string();
		let callers: Vec<_> = (0..count)
			.map(|_| {
				let (flights, fetches, gate, outcome) = (flights.clone(), fetches.clone(), gate.clone(), outcome.clone());
				tokio::spawn(async move {
					flights
						.run("/r/rust/hot.json".to_string(), async {
							fetches.fetch_add(1, Ordering::SeqCst);
							gate.notified().await;
							outcome
						})
						.await
				})
			})
			.collect();
		while flights.waiters(&key) < count - 1 {
			tokio::task::yield_now().await;
		}
		gate.notify_one();
		let mut results = Vec::new();
		for caller in callers {
			results.push(caller.await.unwrap());
		}
		(results, fetches.load(Ordering::SeqCst))
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn vary_singleflight_and_cookies_concurrent_waiters_share_one_fetch() {
		let flights: Flights = Arc::default();
		let (results, fetches) = run_burst(&flights, 10, Ok("page".to_string())).await;
		assert_eq!(fetches, 1, "ten identical callers must cause one fetch");
		assert!(results.iter().all(|result| result == &Ok("page".to_string())));
		assert_eq!(flights.entries(), 0, "nothing may stay in the map after the work finished");
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn vary_singleflight_and_cookies_an_error_is_shared_not_refetched() {
		let flights: Flights = Arc::default();
		let (results, fetches) = run_burst(&flights, 10, Err("Reddit is having issues".to_string())).await;
		assert_eq!(fetches, 1, "waiters must not each retry a failed fetch");
		assert!(results.iter().all(|result| result == &Err("Reddit is having issues".to_string())));
		assert_eq!(flights.entries(), 0);
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn vary_singleflight_and_cookies_different_keys_do_not_wait_for_each_other() {
		let flights: Flights = Arc::default();
		let slow = tokio::spawn({
			let flights = flights.clone();
			async move { flights.run("slow".to_string(), std::future::pending()).await }
		});
		while flights.entries() < 1 {
			tokio::task::yield_now().await;
		}
		assert_eq!(flights.run("fast".to_string(), async { Ok("done".to_string()) }).await, Ok("done".to_string()));
		slow.abort();
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn vary_singleflight_and_cookies_waiters_recover_when_the_leader_is_cancelled() {
		let flights: Flights = Arc::default();
		let key = "/r/rust/hot.json".to_string();
		let leader = tokio::spawn({
			let flights = flights.clone();
			let key = key.clone();
			async move { flights.run(key, std::future::pending()).await }
		});
		while flights.entries() < 1 {
			tokio::task::yield_now().await;
		}
		let waiter = tokio::spawn({
			let flights = flights.clone();
			let key = key.clone();
			async move { flights.run(key, async { Ok("second try".to_string()) }).await }
		});
		while flights.waiters(&key) < 1 {
			tokio::task::yield_now().await;
		}

		leader.abort();

		assert_eq!(waiter.await.unwrap(), Ok("second try".to_string()));
		assert_eq!(flights.entries(), 0);
	}
}
