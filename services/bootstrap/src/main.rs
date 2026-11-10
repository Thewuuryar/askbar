//! `askbar-bootstrap`: see `cli`. Exits 0 when done and 1 with a sentence on stderr when not.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = askbar_bootstrap::cli::main_with(&args) {
        eprintln!("askbar-bootstrap: {e}");
        std::process::exit(1);
    }
}
