// Read-only packaged-update eligibility check using the application's guard.
#[path = "../src-tauri/src/update_bundle.rs"]
mod update_bundle;

fn main() {
    let arguments = std::env::args_os().collect::<Vec<_>>();
    let allowed = arguments.len() == 2
        && update_bundle::preflight(std::path::Path::new(&arguments[1])).is_ok();
    if allowed {
        println!("PASS: packaged app passes source-preserving update preflight");
    } else {
        eprintln!("FAIL: packaged update preflight refused the bundle");
        std::process::exit(1);
    }
}
