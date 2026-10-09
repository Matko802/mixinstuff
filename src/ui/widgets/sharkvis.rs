
use std::io::BufRead;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const FPS: u32 = 30;
const STALE_AFTER: Duration = Duration::from_secs(1);

static FEED: OnceLock<Arc<SharkvisFeed>> = OnceLock::new();

pub struct SharkvisFeed {
    bars: AtomicUsize,
    latest: Arc<Mutex<(Vec<f32>, Instant)>>,
    worker: Mutex<Option<std::process::Child>>,
}

impl SharkvisFeed {
    pub fn global(bars: usize) -> Arc<Self> {
        let feed = FEED.get_or_init(|| Arc::new(Self::new(bars))).clone();
        feed.set_bar_count(bars);
        feed
    }

    fn new(bars: usize) -> Self {
        let bars = bars.clamp(8, 100);
        let feed = Self { bars: AtomicUsize::new(bars), latest: Arc::new(Mutex::new((Vec::new(), Instant::now() - STALE_AFTER * 2))), worker: Mutex::new(None) };
        feed.spawn(bars);
        feed
    }

    pub fn set_bar_count(&self, n: usize) {
        let n = n.clamp(8, 100);
        if self.bars.swap(n, Ordering::SeqCst) == n {
            return;
        }
        self.spawn(n);
    }

    pub fn pull(&self, n: usize) -> Option<Vec<f64>> {
        let (levels, at) = self.latest.lock().unwrap().clone();
        if levels.is_empty() || at.elapsed() > STALE_AFTER {
            return None;
        }
        if levels.len() == n {
            return Some(levels.iter().map(|v| f64::from(*v)).collect());
        }
        Some(resample(&levels, n))
    }

    fn spawn(&self, bars: usize) {
        let mut slot = self.worker.lock().unwrap();
        if let Some(mut old) = slot.take() {
            let _ = old.kill();
            std::thread::spawn(move || {
                let _ = old.wait();
            });
        }
        let child = std::process::Command::new("sharkvis")
            .args(["--raw", "--raw-mode", "bars", "--bars", &bars.to_string(), "--fps", &FPS.to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(err) => {
                tracing::warn!(%err, "sharkvis not available; visualizer stays idle");
                return;
            }
        };
        let stdout = child.stdout.take();
        *slot = Some(child);
        drop(slot);
        let Some(stdout) = stdout else { return };
        let latest = self.latest.clone();
        std::thread::Builder::new()
            .name("musishark-sharkvis".into())
            .spawn(move || {
                let mut reader = std::io::BufReader::new(stdout);
                let mut line = String::new();
                let mut parsed = Vec::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line) {
                        Ok(0) => break,
                        Ok(_) => {}
                        Err(_) => break,
                    }
                    if parse_line(&line, &mut parsed) {
                        *latest.lock().unwrap() = (parsed.clone(), Instant::now());
                    }
                }
            })
            .ok();
    }
}

fn parse_line(line: &str, out: &mut Vec<f32>) -> bool {
    out.clear();
    for part in line.trim().split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Ok(v) = part.parse::<i32>() else { continue };
        out.push((v.clamp(0, 100) as f32) / 100.0);
    }
    !out.is_empty()
}

fn resample(levels: &[f32], n: usize) -> Vec<f64> {
    (0..n).map(|i| f64::from(levels[(i * levels.len()) / n])).collect()
}
