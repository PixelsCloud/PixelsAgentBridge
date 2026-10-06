//! Same internal worker entry as desktop, without rebuilding the GUI for backend tests.
fn main() {
    if pab_desktop_control::ui_worker_entry::run().is_err() {
        std::process::exit(1);
    }
}
