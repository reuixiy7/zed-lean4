use lean_infoview_proxy::{Config, log, run};
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

const USAGE: &str = "usage: lean-infoview-proxy --goals-file <path> [--no-hover] [--no-auto-open] [--no-abbreviations] [--no-status] -- <server> [args...]";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut config = Config::new(PathBuf::from(".lake/InfoView.md"));
    loop {
        match args.next().as_deref() {
            Some("--goals-file") => match args.next() {
                Some(path) => config.goals_file = PathBuf::from(path),
                None => return usage("--goals-file needs a path"),
            },
            Some("--no-hover") => config.hover = false,
            Some("--no-auto-open") => config.auto_open = false,
            Some("--no-abbreviations") => config.abbreviations = false,
            Some("--no-status") => config.status = false,
            Some("--") => break,
            Some("--version") => {
                println!("lean-infoview-proxy {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            other => return usage(&format!("unexpected argument: {other:?}")),
        }
    }
    let Some(server) = args.next() else {
        return usage("missing server command");
    };

    let mut child = match Command::new(&server)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            log(&format!("failed to start {server}: {e}"));
            return ExitCode::FAILURE;
        }
    };

    let server_in = child.stdin.take().expect("piped stdin");
    let server_out = child.stdout.take().expect("piped stdout");
    if let Err(e) = run(
        config,
        std::io::stdin(),
        std::io::stdout(),
        server_in,
        server_out,
    ) {
        log(&e.to_string());
    }

    // Give the server a moment to exit on its own after `exit`, then make sure it is gone.
    for _ in 0..20 {
        if let Ok(Some(status)) = child.try_wait() {
            return ExitCode::from(status.code().unwrap_or(1).clamp(0, 255) as u8);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    ExitCode::SUCCESS
}

fn usage(error: &str) -> ExitCode {
    eprintln!("{USAGE}\n{error}");
    ExitCode::from(2)
}
