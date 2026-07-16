use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if let Some(path) = std::env::var_os("LAVISH_BROWSER_CTL_TRACE")
        && let Ok(mut trace) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
    {
        let line = format!("{}\t{}\n", std::process::id(), args.join("\t"));
        let _ = trace.write_all(line.as_bytes());
    }
    if args
        .first()
        .is_some_and(|argument| argument == "trace-plugin-event")
    {
        return ExitCode::SUCCESS;
    }
    match lavish_browser_cli::run_control(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("lavish-browser-ctl: {error}");
            ExitCode::FAILURE
        }
    }
}
