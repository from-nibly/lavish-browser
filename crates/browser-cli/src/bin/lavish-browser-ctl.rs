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
        let _ = writeln!(trace, "{}\t{}", std::process::id(), args.join("\t"));
    }
    match lavish_browser_cli::run_control(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("lavish-browser-ctl: {error}");
            ExitCode::FAILURE
        }
    }
}
