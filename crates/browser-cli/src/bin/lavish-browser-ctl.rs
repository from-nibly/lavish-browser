use std::process::ExitCode;

fn main() -> ExitCode {
    match lavish_browser_cli::run_control(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("lavish-browser-ctl: {error}");
            ExitCode::FAILURE
        }
    }
}
