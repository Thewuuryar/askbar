//! `askbar-cpa-supervisor <program> [args...]`: run the program under the supervisor (ADR 0037).
//! `ASKBAR_CPA_CONFIG_DIR` (default `/cpa-config`) holds `config.yaml`, `restart-request`, `restart-done` and the export and import
//! request files; `ASKBAR_CPA_AUTH_DIR` (default `/root/.cli-proxy-api`) holds the login files they move.

fn main() {
    let command: Vec<String> = std::env::args().skip(1).collect();
    if command.is_empty() {
        eprintln!("usage: askbar-cpa-supervisor <program> [args...]");
        std::process::exit(2);
    }
    let dir = std::env::var("ASKBAR_CPA_CONFIG_DIR").ok().filter(|d| !d.trim().is_empty()).unwrap_or_else(|| "/cpa-config".into());
    askbar_cpa_supervisor::install_signal_handlers();
    askbar_cpa_supervisor::run(&askbar_cpa_supervisor::Settings::for_dir(&dir, command), &askbar_cpa_supervisor::STOP);
}
