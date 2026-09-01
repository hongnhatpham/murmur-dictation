#[path = "bin/murmur-setup.rs"]
mod setup_cli;

fn main() {
    let mut arguments = std::env::args();
    let _executable = arguments.next();
    if arguments.next().as_deref() == Some("--setup") {
        setup_cli::run_from(arguments.collect());
    }
    if let Err(error) = murmur_core::run() {
        eprintln!("Murmur failed to start: {error}");
        std::process::exit(1);
    }
}
