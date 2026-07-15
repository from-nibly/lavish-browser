use std::process::ExitCode;

fn main() -> ExitCode {
    match lavish_browser_cli::run_lavish_open(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("lavish-open: {error}");
            ExitCode::FAILURE
        }
    }
}
