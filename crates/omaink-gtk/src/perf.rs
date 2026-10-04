//! Opt-in performance instrumentation: set OMAINK_PERF=1 to log timings
//! (stderr) and run the scripted canvas benchmark after the first note opens.

use std::sync::OnceLock;
use std::time::Instant;

pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("OMAINK_PERF").is_some())
}

/// Time a closure and log it when perf mode is on.
pub fn time<T>(label: &str, f: impl FnOnce() -> T) -> T {
    if !enabled() {
        return f();
    }
    let t = Instant::now();
    let out = f();
    eprintln!("[perf] {label}: {:.2} ms", t.elapsed().as_secs_f64() * 1000.0);
    out
}

/// Summarize a set of millisecond samples.
pub fn summarize(label: &str, samples: &mut [f64]) {
    if samples.is_empty() {
        eprintln!("[perf] {label}: no samples");
        return;
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = samples.len();
    let avg = samples.iter().sum::<f64>() / n as f64;
    let p95 = samples[((n as f64 * 0.95) as usize).min(n - 1)];
    let max = samples[n - 1];
    eprintln!("[perf] {label}: n={n} avg={avg:.2} ms p95={p95:.2} ms max={max:.2} ms");
}
