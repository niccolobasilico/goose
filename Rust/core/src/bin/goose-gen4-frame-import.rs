//! Harness CLI: ricostruisce frame storici Gen4 (K=12) dai reading reali di un
//! db openwhoop (tabella heart_rate: 1 riga al secondo con bpm, RR e gravity)
//! e li importa in un goose.sqlite tramite il bridge capture.import_frame_batch,
//! esattamente come farebbe l'app. Poi rilegge feature HR/HRV e il rollup della
//! FC a riposo. Valida parser + ingestione Gen4 su dati reali, senza band.
//!
//! Uso:
//!   cargo run --bin goose-gen4-frame-import -- \
//!     --source-db C:\Users\nicco\.openwhoop\db.sqlite \
//!     --db C:\tmp\gen4-replay.sqlite \
//!     --start "2026-06-02 20:00:00" --end "2026-06-03 08:00:00" \
//!     --date-key 2026-06-03 --timezone Europe/Rome

use goose_core::bridge::handle_bridge_request_json;
use goose_core::protocol::build_gen4_payload_frame;
use goose_core::tool_args::{args, path_value, value};
use rusqlite::Connection;

struct Reading {
    id: i64,
    bpm: i64,
    time: String,
    rr: Vec<u16>,
    gravity: Option<[f32; 3]>,
}

fn bridge(method: &str, args_value: serde_json::Value) -> Result<serde_json::Value, String> {
    let request = serde_json::json!({
        "schema": "goose.bridge.request.v1",
        "request_id": format!("gen4-frame-import-cli-{method}"),
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
    let source_db = path_value(&parsed, "--source-db")
        .map_err(|error| error.to_string())?
        .ok_or("missing required --source-db path (openwhoop db.sqlite)")?;
    let database = path_value(&parsed, "--db")
        .map_err(|error| error.to_string())?
        .ok_or("missing required --db path (goose target sqlite)")?;
    let start = value(&parsed, "--start")
        .map_err(|error| error.to_string())?
        .unwrap_or_else(|| "1970-01-01 00:00:00".to_string());
    let end = value(&parsed, "--end")
        .map_err(|error| error.to_string())?
        .unwrap_or_else(|| "2100-01-01 00:00:00".to_string());
    let date_key = value(&parsed, "--date-key").map_err(|error| error.to_string())?;
    let timezone = value(&parsed, "--timezone")
        .map_err(|error| error.to_string())?
        .unwrap_or_else(|| "Europe/Rome".to_string());
    let chunk_size: usize = value(&parsed, "--chunk")
        .map_err(|error| error.to_string())?
        .map(|raw| raw.parse().map_err(|_| "invalid --chunk"))
        .transpose()?
        .unwrap_or(2000);

    let readings = load_readings(&source_db, &start, &end)?;
    println!(
        "source readings: {} (window {start} .. {end} UTC)",
        readings.len()
    );
    if readings.is_empty() {
        return Err("no readings in the requested window".to_string());
    }

    let database_path = database.display().to_string();
    let mut frames_inserted = 0u64;
    let mut frames_existing = 0u64;
    let mut import_issues = 0u64;
    let mut index = 0usize;
    while index < readings.len() {
        let slice = &readings[index..(index + chunk_size).min(readings.len())];
        index += slice.len();
        let frames: Vec<serde_json::Value> = slice.iter().map(frame_input).collect();
        let result = bridge(
            "capture.import_frame_batch",
            serde_json::json!({
                "database_path": database_path,
                "include_timeline_rows": false,
                "compact_raw_payloads": true,
                "include_results": false,
                "frames": frames,
            }),
        )?;
        frames_inserted += result["frames_inserted"].as_u64().unwrap_or(0);
        frames_existing += result["frames_existing"].as_u64().unwrap_or(0);
        let chunk_issues = result["issues"].as_array().map(Vec::len).unwrap_or(0) as u64;
        if chunk_issues > 0 && import_issues == 0 {
            let issue_texts: Vec<&str> = result["issues"]
                .as_array()
                .map(|issues| {
                    issues
                        .iter()
                        .filter_map(|issue| issue.as_str())
                        .take(3)
                        .collect()
                })
                .unwrap_or_default();
            eprintln!("\nfirst batch issues: {issue_texts:?}");
        }
        import_issues += chunk_issues;
        print!(
            "\rimported {index}/{} frames (inserted {frames_inserted}, existing {frames_existing}, issues {import_issues})",
            readings.len()
        );
    }
    println!();
    if import_issues > 0 {
        return Err(format!("{import_issues} frame import issues, aborting"));
    }

    // Riletture come l'app.
    let window_start = format!("{}Z", start.replace(' ', "T"));
    let window_end = format!("{}Z", end.replace(' ', "T"));

    let hr_report = bridge(
        "metrics.heart_rate_features",
        serde_json::json!({
            "database_path": database_path,
            "start": window_start,
            "end": window_end,
        }),
    )?;
    let hr_features = hr_report["features"].as_array().cloned().unwrap_or_default();
    let mut bpm_values: Vec<f64> = hr_features
        .iter()
        .filter_map(|feature| feature["heart_rate_bpm"].as_f64())
        .collect();
    bpm_values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let bpm_summary = if bpm_values.is_empty() {
        "none".to_string()
    } else {
        format!(
            "min {:.0} / median {:.0} / max {:.0}",
            bpm_values[0],
            bpm_values[bpm_values.len() / 2],
            bpm_values[bpm_values.len() - 1],
        )
    };
    println!("heart_rate_features: {} (bpm {bpm_summary})", hr_features.len());

    let hrv_report = bridge(
        "metrics.hrv_features",
        serde_json::json!({
            "database_path": database_path,
            "start": window_start,
            "end": window_end,
        }),
    )?;
    let hrv_features = hrv_report["features"].as_array().cloned().unwrap_or_default();
    let rr_total: usize = hrv_features
        .iter()
        .filter_map(|feature| feature["rr_intervals_ms"].as_array().map(Vec::len))
        .sum();
    println!(
        "hrv_features: {} frames with RR ({} RR intervals total, rmssd {})",
        hrv_features.len(),
        rr_total,
        hrv_report["rmssd_ms"]
            .as_f64()
            .map(|rmssd| format!("{rmssd:.1} ms"))
            .unwrap_or_else(|| "n/a".to_string()),
    );

    if let Some(date_key) = date_key {
        let rollup = bridge(
            "metrics.resting_hr_daily_rollup",
            serde_json::json!({
                "database_path": database_path,
                "date_key": date_key,
                "timezone": timezone,
                "start": window_start,
                "end": window_end,
                "write_metric": true,
            }),
        )?;
        println!(
            "resting_hr_daily_rollup {date_key}: resting_hr={} sample_count={} written={}",
            rollup["resting_hr_bpm"],
            rollup["sample_count"],
            rollup["metric_written"],
        );

        let recovery_rows = bridge(
            "metrics.daily_recovery_metrics",
            serde_json::json!({
                "database_path": database_path,
                "start_time_unix_ms": 0i64,
                "end_time_unix_ms": 4102444800000i64,
            }),
        )?;
        let rows = recovery_rows["metrics"]
            .as_array()
            .or_else(|| recovery_rows.as_array())
            .cloned()
            .unwrap_or_default();
        println!("daily_recovery_metrics rows: {}", rows.len());
        for row in &rows {
            println!(
                "  {} resting_hr={} hrv_rmssd={}",
                row["date_key"], row["resting_hr_bpm"], row["hrv_rmssd_ms"],
            );
        }
    }

    Ok(())
}

fn load_readings(
    source_db: &std::path::Path,
    start: &str,
    end: &str,
) -> Result<Vec<Reading>, String> {
    let connection = Connection::open(source_db).map_err(|error| error.to_string())?;
    let mut statement = connection
        .prepare(
            "SELECT id, bpm, time, rr_intervals, sensor_data FROM heart_rate \
             WHERE bpm > 0 AND time >= ?1 AND time <= ?2 ORDER BY time",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([start, end], |row| {
            let rr_text: String = row.get(3)?;
            let sensor_text: Option<String> = row.get(4)?;
            Ok(Reading {
                id: row.get(0)?,
                bpm: row.get(1)?,
                time: row.get(2)?,
                rr: rr_text
                    .split(',')
                    .filter_map(|token| token.trim().parse::<u16>().ok())
                    .take(4)
                    .collect(),
                gravity: sensor_text.and_then(|text| {
                    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
                    let gravity = value["accel_gravity"].as_array()?;
                    Some([
                        gravity.first()?.as_f64()? as f32,
                        gravity.get(1)?.as_f64()? as f32,
                        gravity.get(2)?.as_f64()? as f32,
                    ])
                }),
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

/// Ricostruisce il payload K=12 nel layout Harvard verificato (offsets in data,
/// dopo l'header [type, k, status]): sequence[0:4], unix[4:8], bpm[14],
/// rr_count[15], 4 slot RR u16 LE [16:24], gravity 3 x f32 LE [33:45].
fn frame_input(reading: &Reading) -> serde_json::Value {
    let mut payload = vec![0u8; 3 + 77];
    payload[0] = 47; // HISTORICAL_DATA
    payload[1] = 12; // K version with DSP/motion fields
    payload[2] = 1;
    payload[3..7].copy_from_slice(&(reading.id as u32).to_le_bytes());
    payload[7..11].copy_from_slice(&(naive_utc_to_unix(&reading.time) as u32).to_le_bytes());
    payload[17] = reading.bpm.clamp(0, 255) as u8;
    payload[18] = reading.rr.len() as u8;
    for (slot, rr) in reading.rr.iter().enumerate() {
        let offset = 19 + slot * 2;
        payload[offset..offset + 2].copy_from_slice(&rr.to_le_bytes());
    }
    if let Some(gravity) = reading.gravity {
        payload[36..40].copy_from_slice(&gravity[0].to_le_bytes());
        payload[40..44].copy_from_slice(&gravity[1].to_le_bytes());
        payload[44..48].copy_from_slice(&gravity[2].to_le_bytes());
    }

    let frame_hex = hex::encode(build_gen4_payload_frame(&payload));
    let evidence_id = format!("gen4-replay.{}", reading.id);
    serde_json::json!({
        "evidence_id": evidence_id,
        "frame_id": format!("{evidence_id}.frame.0"),
        "source": "capture.sqlite.gen4-replay",
        "captured_at": format!("{}Z", reading.time.replace(' ', "T")),
        "device_model": "Gen4 Band",
        "frame_hex": frame_hex,
        "sensitivity": "user-owned-capture",
        "device_type": "GEN4",
    })
}

/// 'YYYY-MM-DD HH:MM:SS' (UTC naive, come scrive openwhoop) -> unix seconds.
fn naive_utc_to_unix(text: &str) -> i64 {
    let bytes = text.as_bytes();
    let digits = |range: std::ops::Range<usize>| -> i64 {
        text.get(range)
            .and_then(|token| token.parse::<i64>().ok())
            .unwrap_or(0)
    };
    if bytes.len() < 19 {
        return 0;
    }
    let (year, month, day) = (digits(0..4), digits(5..7), digits(8..10));
    let (hour, minute, second) = (digits(11..13), digits(14..16), digits(17..19));
    days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second
}

/// Howard Hinnant's days_from_civil.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let adjusted_year = if month <= 2 { year - 1 } else { year };
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}
