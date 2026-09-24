use lavish_browser_cli::herdr::run_herdr_sync;

fn main() {
    if let Err(error) = run_herdr_sync(std::env::args().skip(1).collect()) {
        eprintln!("lavish-browser-herdr-sync: {error}");
        std::process::exit(1);
    }
}
