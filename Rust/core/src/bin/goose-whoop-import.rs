//! Harness CLI: importa un murmur-history.json in un goose.sqlite via bridge JSON
//! e rilegge le metriche come farebbe l'app. Permette di validare l'intera catena
//! converter -> bridge -> store -> read su desktop, senza Xcode.

use goose_core::bridge::handle_bridge_request_json;
use goose_core::tool_args::{args, path_value, value};

fn bridge(method: &str, args_value: serde_json::Value) -> Result<serde_json::Value, String> {
    let request = serde_json::json!({
        "schema": "goose.bridge.request.v1",
        "request_id": format!("whoop-import-cli-{method}"),
        "method": method,
        "args": args_value,
    });
    let raw = handle_bridge_request_json(&request.to_string());
    let response: serde_json::Value =
        serde_json::from_str(&raw).map_err(|error| error.to_string())?;
    if response["ok"].as_bool() != Some(true) {
        return Err(response["error"].to_string());
    }
    Ok(response["result"].clone())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let parsed = args();
    let database = path_value(&parsed, "--db")
        .map_err(|error| error.to_string())?
        .ok_or("missing required --db path")?;
    let input = path_value(&parsed, "--in")
        .map_err(|error| error.to_string())?
        .ok_or("missing required --in murmur-history.json path")?;
    let chunk_size: usize = value(&parsed, "--chunk")
        .map_err(|error| error.to_string())?
        .map(|raw| raw.parse().map_err(|_| "invalid --chunk"))
        .transpose()?
        .unwrap_or(50);

    let text = std::fs::read_to_string(&input).map_err(|error| error.to_string())?;
    let payload: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| error.to_string())?;
    if payload["schema"].as_str() != Some("murmur.history.v1") {
        return Err(format!(
            "unsupported schema: {}",
            payload["schema"]
        ));
    }

    let database_path = database.display().to_string();
    let mut totals = serde_json::Map::new();
    for section in ["recovery", "activity", "sleep", "workouts"] {
        let entries = payload[section].as_array().cloned().unwrap_or_default();
        let mut index = 0usize;
        while index < entries.len() {
            let chunk: Vec<_> = entries[index..(index + chunk_size).min(entries.len())].to_vec();
            index += chunk.len();
            let result = bridge(
                "import.whoop_history_batch",
                serde_json::json!({
                    "database_path": database_path,
                    section: chunk,
                }),
            )?;
            if let Some(object) = result.as_object() {
                for (key, value) in object {
                    if let Some(count) = value.as_u64() {
                        let slot = totals.entry(key.clone()).or_insert(serde_json::json!(0));
                        *slot = serde_json::json!(slot.as_u64().unwrap_or(0) + count);
                    }
                }
            }
        }
        println!("{section}: {} entries imported", entries.len());
    }
    println!(
        "import totals: {}",
        serde_json::Value::Object(totals.clone())
    );

    // Riletture come l'app
    let recovery_rows = bridge(
        "metrics.daily_recovery_metrics",
        serde_json::json!({
            "database_path": database_path,
            "start_time_unix_ms": 0i64,
            "end_time_unix_ms": 4102444800000i64,
        }),
    )?;
    let activity_rows = bridge(
        "metrics.daily_activity_metrics",
        serde_json::json!({
            "database_path": database_path,
            "start_time_unix_ms": 0i64,
            "end_time_unix_ms": 4102444800000i64,
        }),
    )?;
    let sessions = bridge(
        "activity.list_sessions",
        serde_json::json!({
            "database_path": database_path,
            "start_time_unix_ms": 0i64,
            "end_time_unix_ms": 4102444800000i64,
        }),
    )?;

    let count = |value: &serde_json::Value| -> usize {
        value
            .as_array()
            .map(Vec::len)
            .or_else(|| value["metrics"].as_array().map(Vec::len))
            .or_else(|| value["sessions"].as_array().map(Vec::len))
            .unwrap_or(0)
    };
    println!(
        "read back: daily_recovery={} daily_activity={} activity_sessions={}",
        count(&recovery_rows),
        count(&activity_rows),
        count(&sessions),
    );
    Ok(())
}
