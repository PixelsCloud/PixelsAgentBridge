//! Source acceptance probe; not a public tool or installed executable.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let scope = match args.first().map(String::as_str) {
        Some("installed") => pab_protocol::AppListScope::Installed,
        Some("running") => pab_protocol::AppListScope::Running,
        _ => return Err("usage: apps_probe installed|running [search]".into()),
    };
    let query = pab_protocol::AppListRequest {
        scope,
        search: args.get(1).cloned().unwrap_or_default(),
        limit: 200,
    };
    let result = pab_desktop_control::apps::list(&query)?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
