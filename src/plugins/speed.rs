//! `telemetrix.speed_multi`: a time-based speed test over several parallel
//! connections. One connection cannot fill a fast line (TCP ramp-up, one
//! TLS stream per core), so N threads loop over requests and count every
//! byte in one shared counter, and the Lua thread samples that counter.

use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use ureq::Agent;

use super::host_api::Transfer;

const TICK: Duration = Duration::from_millis(100);
const PROGRESS_EVERY: Duration = Duration::from_millis(250);
/// Time after the measurement for the threads to notice the stop flag.
const WIND_DOWN: Duration = Duration::from_millis(1500);
const STREAM_STACK: usize = 256 * 1024;
const BUFFER: usize = 64 * 1024;
pub const MAX_STREAMS: usize = 16;

#[derive(Clone, Debug, PartialEq)]
pub struct Multi {
    pub upload: bool,
    pub url: String,
    pub streams: usize,
    pub seconds: f64,
    pub warmup: f64,
    /// Bytes per upload request.
    pub piece_bytes: u64,
}

impl Multi {
    pub fn check(&self) -> Result<(), String> {
        if !self.url.starts_with("https://") {
            return Err("speed tests accept only https:// addresses".into());
        }
        if !(1..=MAX_STREAMS).contains(&self.streams) {
            return Err(format!("streams must be 1..{MAX_STREAMS}"));
        }
        if !(self.seconds > 0.0 && self.seconds.is_finite()) {
            return Err("seconds must be above 0".into());
        }
        if !(self.warmup >= 0.0 && self.warmup < self.seconds) {
            return Err("warmup must be 0 or more and below seconds".into());
        }
        if self.upload && self.piece_bytes == 0 {
            return Err("piece_bytes must be above 0".into());
        }
        Ok(())
    }
}

/// The URL with `r=<n>` added, so no cache between us and the server
/// answers a repeated request.
pub fn cache_bust(url: &str, n: u64) -> String {
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}r={n}")
}

/// Zeros for one upload request; stops at `left` bytes or when the test stops.
struct Zeros {
    left: u64,
    stop: Arc<AtomicBool>,
    counter: Arc<AtomicU64>,
}

impl Read for Zeros {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.left == 0 || self.stop.load(Ordering::Relaxed) {
            return Ok(0);
        }
        let n = buf.len().min(BUFFER).min(self.left as usize);
        buf[..n].fill(0);
        self.left -= n as u64;
        self.counter.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

struct Shared {
    counter: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    alive: AtomicUsize,
    error: Mutex<Option<String>>,
    /// No request may run past this moment.
    hard_end: Instant,
}

impl Shared {
    fn fail(&self, e: String) {
        let mut slot = self.error.lock().unwrap_or_else(|p| p.into_inner());
        slot.get_or_insert(e);
    }
}

fn stream(agent: &Agent, spec: &Multi, shared: &Shared, id: u64) {
    let mut buf = vec![0u8; BUFFER];
    let mut n = 0u64;
    while !shared.stop.load(Ordering::Relaxed) {
        let left = shared.hard_end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let url = cache_bust(&spec.url, id * 1_000_000 + n);
        n += 1;
        let result = if spec.upload {
            let body = Zeros {
                left: spec.piece_bytes,
                stop: shared.stop.clone(),
                counter: shared.counter.clone(),
            };
            // Ookla servers answer a chunked upload with HTTP 500 after 30 s,
            // so the length is declared; stopping early then ends the request
            // with an error, which is ignored below.
            agent
                .post(&url)
                .header("Content-Length", spec.piece_bytes.to_string())
                .config()
                .timeout_global(Some(left))
                .build()
                .send(ureq::SendBody::from_owned_reader(body))
        } else {
            agent
                .get(&url)
                .config()
                .timeout_global(Some(left))
                .build()
                .call()
        };
        let mut response = match result {
            Ok(r) => r,
            // Stopping mid-upload can end the request with an error; that is expected.
            Err(_) if shared.stop.load(Ordering::Relaxed) => break,
            Err(e) => {
                shared.fail(e.to_string());
                break;
            }
        };
        let status = response.status().as_u16();
        if status >= 400 || (!spec.upload && status != 200) {
            shared.fail(format!("the server answered HTTP {status}"));
            break;
        }
        let mut reader = response.body_mut().with_config().limit(u64::MAX).reader();
        while !shared.stop.load(Ordering::Relaxed) {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(read) if !spec.upload => {
                    shared.counter.fetch_add(read as u64, Ordering::Relaxed);
                }
                Ok(_) => {}
            }
        }
    }
    shared.alive.fetch_sub(1, Ordering::Relaxed);
}

/// Samples `counter` every 100 ms for `seconds`. The rate counts only what
/// arrived after `warmup`, so connection set-up and TCP ramp-up do not
/// lower it. `progress(mbps, seconds)` gets the rate since the warm-up at
/// most 4 times a second; an error from it ends the test. Ends early when
/// no stream is left.
pub fn sample(
    counter: &AtomicU64,
    alive: &AtomicUsize,
    seconds: Duration,
    warmup: Duration,
    mut progress: impl FnMut(f64, f64) -> Result<(), String>,
) -> Result<Transfer, String> {
    let start = Instant::now();
    let mut warm: Option<(Instant, u64)> = None;
    let mut last_progress: Option<Instant> = None;
    loop {
        let now = Instant::now();
        let t = now - start;
        if warm.is_none() && t >= warmup {
            warm = Some((now, counter.load(Ordering::Relaxed)));
        }
        if t >= seconds || alive.load(Ordering::Relaxed) == 0 {
            break;
        }
        if let Some((wt, wb)) = warm {
            let since = now - wt;
            if since >= TICK && last_progress.is_none_or(|p| p.elapsed() >= PROGRESS_EVERY) {
                let bytes = counter.load(Ordering::Relaxed) - wb;
                progress(mbps(bytes, since.as_secs_f64()), t.as_secs_f64())?;
                last_progress = Some(Instant::now());
            }
        }
        thread::sleep(TICK.min(seconds.saturating_sub(start.elapsed())));
    }
    let end = Instant::now();
    let (wt, wb) = warm.unwrap_or((start, 0));
    Ok(Transfer {
        bytes: counter.load(Ordering::Relaxed) - wb,
        seconds: (end - wt).as_secs_f64(),
    })
}

pub fn mbps(bytes: u64, seconds: f64) -> f64 {
    if seconds > 0.0 {
        bytes as f64 * 8.0 / seconds / 1_000_000.0
    } else {
        0.0
    }
}

fn agent(streams: usize) -> Agent {
    Agent::config_builder()
        .http_status_as_error(false)
        .max_idle_connections(streams.max(2))
        .max_idle_connections_per_host(streams.max(2))
        .input_buffer_size(BUFFER)
        .output_buffer_size(BUFFER)
        .user_agent(concat!("telemetrix/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// Runs one direction of a speed test. `cap` is what is left of the
/// plugin's call time; the test is shortened to end inside it.
pub fn run(
    spec: &Multi,
    cap: Option<Duration>,
    progress: impl FnMut(f64, f64) -> Result<(), String>,
) -> Result<Transfer, String> {
    spec.check()?;
    let mut seconds = Duration::from_secs_f64(spec.seconds);
    if let Some(cap) = cap {
        seconds = seconds.min(cap.saturating_sub(WIND_DOWN));
    }
    let warmup = Duration::from_secs_f64(spec.warmup).min(seconds / 2);
    if seconds.is_zero() {
        return Err("no call time left for the speed test".into());
    }
    let agent = agent(spec.streams);
    let shared = Arc::new(Shared {
        counter: Arc::new(AtomicU64::new(0)),
        stop: Arc::new(AtomicBool::new(false)),
        alive: AtomicUsize::new(spec.streams),
        error: Mutex::new(None),
        hard_end: Instant::now() + seconds + WIND_DOWN / 2,
    });
    let mut handles = Vec::new();
    for id in 0..spec.streams {
        let (agent, spec, mine) = (agent.clone(), spec.clone(), shared.clone());
        let spawned = thread::Builder::new()
            .name("speed test".into())
            .stack_size(STREAM_STACK)
            .spawn(move || stream(&agent, &spec, &mine, id as u64));
        match spawned {
            Ok(h) => handles.push(h),
            Err(e) => {
                shared.alive.fetch_sub(1, Ordering::Relaxed);
                shared.fail(format!("cannot start a speed-test thread: {e}"));
            }
        }
    }
    let result = sample(&shared.counter, &shared.alive, seconds, warmup, progress);
    shared.stop.store(true, Ordering::Relaxed);
    for h in handles {
        let _ = h.join();
    }
    let transfer = result?;
    if transfer.bytes == 0 {
        let error = shared
            .error
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        return Err(error.unwrap_or_else(|| "no data was moved".into()));
    }
    Ok(transfer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_buster_respects_an_existing_query() {
        assert_eq!(
            cache_bust("https://h/download?size=25", 7),
            "https://h/download?size=25&r=7"
        );
        assert_eq!(
            cache_bust("https://h/upload", 1_000_002),
            "https://h/upload?r=1000002"
        );
    }

    #[test]
    fn checks_the_arguments() {
        let ok = Multi {
            upload: false,
            url: "https://h/x".into(),
            streams: 4,
            seconds: 3.0,
            warmup: 0.5,
            piece_bytes: 1,
        };
        assert!(ok.check().is_ok());
        for bad in [
            Multi {
                url: "http://h/x".into(),
                ..ok.clone()
            },
            Multi {
                streams: 0,
                ..ok.clone()
            },
            Multi {
                streams: 17,
                ..ok.clone()
            },
            Multi {
                seconds: 0.0,
                ..ok.clone()
            },
            Multi {
                warmup: 3.0,
                ..ok.clone()
            },
            Multi {
                upload: true,
                piece_bytes: 0,
                ..ok.clone()
            },
        ] {
            assert!(bad.check().is_err(), "{bad:?}");
        }
    }

    /// A fake stream: 100 KB every 10 ms (80 Mbps), but ten times slower in
    /// the first 300 ms, like a TCP connection that is still ramping up.
    fn fake_source(counter: Arc<AtomicU64>, stop: Arc<AtomicBool>) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let start = Instant::now();
            while !stop.load(Ordering::Relaxed) {
                let chunk = if start.elapsed() < Duration::from_millis(300) {
                    10_000
                } else {
                    100_000
                };
                counter.fetch_add(chunk, Ordering::Relaxed);
                thread::sleep(Duration::from_millis(10));
            }
        })
    }

    #[test]
    fn warmup_is_left_out_and_the_test_ends_on_time() {
        let counter = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let source = fake_source(counter.clone(), stop.clone());
        let alive = AtomicUsize::new(1);
        let mut seen = Vec::new();
        let start = Instant::now();
        let t = sample(
            &counter,
            &alive,
            Duration::from_millis(1500),
            Duration::from_millis(500),
            |rate, secs| {
                seen.push((rate, secs));
                Ok(())
            },
        )
        .unwrap();
        let took = start.elapsed();
        stop.store(true, Ordering::Relaxed);
        source.join().unwrap();
        assert!(
            took >= Duration::from_millis(1500) && took < Duration::from_millis(1800),
            "{took:?}"
        );
        assert!(
            (t.seconds - 1.0).abs() < 0.15,
            "measured after the warm-up: {}",
            t.seconds
        );
        // Sleep granularity makes the source slower than 80 Mbps, never faster,
        // and the slow start must not pull the result down to the average.
        let rate = t.mbps();
        assert!(rate > 40.0 && rate <= 82.0, "{rate}");
        assert!(
            !seen.is_empty() && seen.len() <= 5,
            "at most 4 a second: {seen:?}"
        );
        assert!(
            seen.iter().all(|(_, s)| *s >= 0.5),
            "no progress before the warm-up"
        );
    }

    #[test]
    fn rate_math_counts_only_after_the_warmup() {
        assert_eq!(mbps(25_000_000, 2.0), 100.0);
        assert_eq!(mbps(1, 0.0), 0.0);
    }

    #[test]
    fn no_stream_left_ends_early_and_progress_errors_stop() {
        let counter = AtomicU64::new(0);
        let start = Instant::now();
        let t = sample(
            &counter,
            &AtomicUsize::new(0),
            Duration::from_secs(5),
            Duration::ZERO,
            |_, _| Ok(()),
        )
        .unwrap();
        assert!(start.elapsed() < Duration::from_millis(200));
        assert_eq!(t.bytes, 0);
        let counter = AtomicU64::new(0);
        let r = sample(
            &counter,
            &AtomicUsize::new(1),
            Duration::from_secs(5),
            Duration::ZERO,
            |_, _| Err("stop".into()),
        );
        assert_eq!(r.unwrap_err(), "stop");
    }
}
