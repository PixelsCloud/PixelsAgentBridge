//! Source acceptance probe; not a public tool or installed executable.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "action") {
        let request: pab_protocol::AppActionRequest =
            serde_json::from_str(args.get(1).ok_or("missing action JSON")?)?;
        match pab_desktop_control::apps::act(&request) {
            Ok(value) => println!("{}", serde_json::to_string_pretty(&value)?),
            Err(error) => {
                println!("{}", serde_json::to_string(&error)?);
                return Err(error.into());
            }
        }
        return Ok(());
    }
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
