//! `mecheval-grade` — load a task JSON + a candidate `.vcad` and print the
//! grader's verdict as a (partial) run blob.
//!
//! Usage:
//!
//! ```text
//! mecheval-grade <task.json> <candidate.vcad>
//! ```

use mecheval_grader::{grade, task::load_task};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let task_path = match args.next() {
        Some(p) => PathBuf::from(p),
        None => {
            eprintln!("usage: mecheval-grade <task.json> <candidate.vcad>");
            return ExitCode::from(2);
        }
    };
    let vcad_path = match args.next() {
        Some(p) => PathBuf::from(p),
        None => {
            eprintln!("usage: mecheval-grade <task.json> <candidate.vcad>");
            return ExitCode::from(2);
        }
    };

    let task_bytes = match std::fs::read(&task_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("could not read task {:?}: {}", task_path, e);
            return ExitCode::from(2);
        }
    };
    let task = match load_task(&task_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("could not parse task {:?}: {}", task_path, e);
            return ExitCode::from(2);
        }
    };

    let task_dir = task_path.parent().unwrap_or_else(|| Path::new("."));
    match grade(&task, &task_bytes, &vcad_path, task_dir) {
        Ok(blob) => {
            let pretty = serde_json::to_string_pretty(&blob).expect("serialize blob");
            println!("{}", pretty);
            if blob.summary.passed {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(e) => {
            eprintln!("grader error: {}", e);
            ExitCode::from(2)
        }
    }
}
