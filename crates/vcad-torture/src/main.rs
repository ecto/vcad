//! Torture-track runner.
//!
//! Modes:
//! - `vcad-torture run [--subset pr|full] [--jobs N] [--timeout-secs S]
//!    [--json PATH] [--md PATH] [--baseline PATH] [--check] [--write-baseline PATH]`
//!   — orchestrate the corpus with per-case subprocess isolation.
//! - `vcad-torture run-case <id>` — execute one case in-process and print a
//!   JSON `CaseResult` line (internal; spawned by `run`).
//! - `vcad-torture list [--subset pr|full]` — print case ids.

use std::io::Write as _;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use vcad_torture::{build_corpus, execute_case, Case, CaseResult, Class, Scorecard};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("run-case") => run_case(&args[1..]),
        Some("run") => run(&args[1..]),
        Some("list") => list(&args[1..]),
        _ => {
            eprintln!("usage: vcad-torture <run|run-case|list> [options]");
            2
        }
    };
    std::process::exit(code);
}

fn flag_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

fn select_subset(args: &[String]) -> (Vec<Case>, String) {
    let subset = flag_value(args, "--subset").unwrap_or("full").to_string();
    let mut corpus = build_corpus();
    if subset == "pr" {
        corpus.retain(|c| c.pr_subset);
    }
    (corpus, subset)
}

fn list(args: &[String]) -> i32 {
    let (corpus, _) = select_subset(args);
    for c in &corpus {
        println!("{}\t{}", c.category.name(), c.id);
    }
    0
}

fn run_case(args: &[String]) -> i32 {
    let Some(id) = args.first() else {
        eprintln!("run-case: missing case id");
        return 2;
    };
    let corpus = build_corpus();
    let Some(case) = corpus.iter().find(|c| &c.id == id) else {
        eprintln!("run-case: unknown case id {id}");
        return 2;
    };
    // Panics unwind out of execute_case and abort the process with a
    // non-zero exit; the parent classifies that as a crash.
    let result = execute_case(case);
    println!("{}", serde_json::to_string(&result).unwrap());
    0
}

/// Run one case as an isolated subprocess with a timeout.
fn run_one_isolated(exe: &std::path::Path, case: &Case, timeout: Duration) -> CaseResult {
    let child = Command::new(exe)
        .arg("run-case")
        .arg(&case.id)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            return CaseResult {
                id: case.id.clone(),
                class: Class::Crash,
                detail: format!("failed to spawn subprocess: {e}"),
            }
        }
    };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let out = child.stdout.take();
                let mut stdout = String::new();
                if let Some(mut o) = out {
                    use std::io::Read;
                    let _ = o.read_to_string(&mut stdout);
                }
                if status.success() {
                    if let Some(line) = stdout.lines().last() {
                        if let Ok(r) = serde_json::from_str::<CaseResult>(line) {
                            return r;
                        }
                    }
                    return CaseResult {
                        id: case.id.clone(),
                        class: Class::Crash,
                        detail: "subprocess exited 0 without a result line".into(),
                    };
                }
                let mut stderr = String::new();
                if let Some(mut e) = child.stderr.take() {
                    use std::io::Read;
                    let _ = e.read_to_string(&mut stderr);
                }
                let lines: Vec<&str> = stderr.lines().collect();
                // Prefer the panic location + message over whatever stderr
                // ended with (usually the RUST_BACKTRACE hint).
                let last = if let Some(i) = lines.iter().position(|l| l.contains("panicked at")) {
                    lines[i..]
                        .iter()
                        .take(2)
                        .map(|l| l.trim())
                        .collect::<Vec<_>>()
                        .join(" — ")
                } else {
                    lines
                        .iter()
                        .rfind(|l| !l.trim().is_empty())
                        .copied()
                        .unwrap_or("")
                        .to_string()
                }
                .chars()
                .take(300)
                .collect::<String>();
                return CaseResult {
                    id: case.id.clone(),
                    class: Class::Crash,
                    detail: format!("subprocess exited with {status}: {last}"),
                };
            }
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return CaseResult {
                        id: case.id.clone(),
                        class: Class::Timeout,
                        detail: format!("exceeded {}s", timeout.as_secs()),
                    };
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                return CaseResult {
                    id: case.id.clone(),
                    class: Class::Crash,
                    detail: format!("wait failed: {e}"),
                }
            }
        }
    }
}

fn run(args: &[String]) -> i32 {
    let (corpus, subset) = select_subset(args);
    let timeout = Duration::from_secs(
        flag_value(args, "--timeout-secs")
            .and_then(|v| v.parse().ok())
            .unwrap_or(20),
    );
    let jobs: usize = flag_value(args, "--jobs")
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
        });

    let exe = std::env::current_exe().expect("current_exe");
    let started = Instant::now();
    eprintln!(
        "torture track: {} cases (subset={subset}), {jobs} jobs, {}s timeout",
        corpus.len(),
        timeout.as_secs()
    );

    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<CaseResult>> = Mutex::new(Vec::with_capacity(corpus.len()));
    let done = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..jobs {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(case) = corpus.get(i) else { break };
                let r = run_one_isolated(&exe, case, timeout);
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                if r.class != Class::Pass {
                    eprintln!(
                        "[{n}/{}] {} → {} {}",
                        corpus.len(),
                        r.id,
                        r.class.name(),
                        r.detail
                    );
                } else if n.is_multiple_of(50) {
                    eprintln!("[{n}/{}] …", corpus.len());
                }
                results.lock().unwrap().push(r);
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort_by(|a, b| a.id.cmp(&b.id));

    let card = Scorecard::from_results(&subset, &results, &corpus);
    let totals = card.totals();
    eprintln!(
        "done in {:.1}s: {} pass / {} refusal / {} bad-geometry / {} timeout / {} crash ({:.1}% pass)",
        started.elapsed().as_secs_f64(),
        totals.pass,
        totals.graceful_refusal,
        totals.bad_geometry,
        totals.timeout,
        totals.crash,
        totals.pass_rate()
    );

    if let Some(path) = flag_value(args, "--json") {
        std::fs::write(path, serde_json::to_string_pretty(&card).unwrap()).expect("write --json");
    }
    if let Some(path) = flag_value(args, "--write-baseline") {
        std::fs::write(path, serde_json::to_string_pretty(&card).unwrap())
            .expect("write --write-baseline");
        eprintln!("baseline written to {path}");
    }
    if let Some(path) = flag_value(args, "--md") {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("open --md");
        writeln!(f, "## Kernel torture track ({subset} subset)\n").unwrap();
        writeln!(f, "{}", card.to_markdown()).unwrap();
    }

    let mut failed = false;
    if let Some(path) = flag_value(args, "--baseline") {
        let baseline: Scorecard =
            serde_json::from_str(&std::fs::read_to_string(path).expect("read --baseline"))
                .expect("parse --baseline");
        let (mut regressions, improvements) = card.diff_baseline(&baseline);
        // Confirm regressions with two isolated re-runs: a case only counts
        // if every attempt stays worse than the baseline. The kernel has a
        // known nondeterminism (HashMap iteration order in a few paths) that
        // can flip a borderline case between runs; the baseline policy is to
        // record such cases at their WORSE class, and this retry filters the
        // residual transient flips.
        regressions.retain(|line| {
            let id = line.split(':').next().unwrap_or("");
            let Some(case) = corpus.iter().find(|c| c.id == id) else {
                return true;
            };
            let base_rank = baseline.cases[id].rank();
            let confirmed = (0..2).all(|_| {
                run_one_isolated(&exe, case, timeout).class.rank() > base_rank
            });
            if !confirmed {
                eprintln!("  ~ {id}: regression did not reproduce on retry (flaky — record its worse class in the baseline)");
            }
            confirmed
        });
        if !improvements.is_empty() {
            eprintln!("\n{} improvement(s) vs baseline:", improvements.len());
            for l in &improvements {
                eprintln!("  ✓ {l}");
            }
            eprintln!("(refresh the baseline to lock these in: --write-baseline {path})");
        }
        if !regressions.is_empty() {
            eprintln!("\n{} REGRESSION(S) vs baseline:", regressions.len());
            for l in &regressions {
                eprintln!("  ✗ {l}");
            }
            if args.iter().any(|a| a == "--check") {
                failed = true;
            }
        } else {
            eprintln!("no regressions vs baseline");
        }
    }
    i32::from(failed)
}
