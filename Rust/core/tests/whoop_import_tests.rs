use goose_core::bridge::{BridgeResponse, handle_bridge_request_json};

fn request(value: serde_json::Value) -> BridgeResponse {
    serde_json::from_str(&handle_bridge_request_json(&value.to_string())).unwrap()
}

fn sample_batch_args(db_path: &str) -> serde_json::Value {
    serde_json::json!({
        "database_path": db_path,
        "recovery": [
            {
                "import_key": "wh-rec-2024-11-29",
                "date_key": "2024-11-29",
                "timezone": "+01:00",
                "start_time_unix_ms": 1732834800000i64,
                "end_time_unix_ms": 1732921199000i64,
                "resting_hr_bpm": 53.0,
                "hrv_rmssd_ms": 60.0,
                "respiratory_rate_rpm": 12.9,
                "oxygen_saturation_percent": 95.4,
                "skin_temperature_delta_c": 0.4,
                "imported_score_0_to_100": 53.0,
                "confidence": 0.9
            }
        ],
        "activity": [
            {
                "import_key": "wh-act-2024-11-29",
                "date_key": "2024-11-29",
                "timezone": "+01:00",
                "start_time_unix_ms": 1732834800000i64,
                "end_time_unix_ms": 1732921199000i64,
                "total_kcal": 2229.0,
                "imported_strain": 10.8,
                "confidence": 0.8
            }
        ],
        "sleep": [
            {
                "import_key": "wh-sleep-2024-11-28T23:08:41",
                "platform_record_id": "wh-sleep-2024-11-28T23:08:41",
                "start_time_unix_ms": 1732831721000i64,
                "end_time_unix_ms": 1732859032000i64,
                "timezone": "+01:00",
                "minutes_by_stage": {
                    "light": 168.0,
                    "deep": 103.0,
                    "rem": 148.0,
                    "awake": 36.0
                },
                "is_nap": false,
                "imported_performance_percent": 70.0,
                "imported_efficiency_percent": 92.0,
                "imported_regularity_percent": 83.0,
                "confidence": 0.9
            }
        ],
        "workouts": [
            {
                "import_key": "wh-wkt-2024-11-29T08:52:47",
                "start_time_unix_ms": 1732866767000i64,
                "end_time_unix_ms": 1732870180000i64,
                "activity_type": "weightlifting",
                "external_activity_type_name": "Sollevamento pesi",
                "custom_label": "Sollevamento pesi",
                "kcal": 135.0,
                "hr_max": 147.0,
                "hr_avg": 96.0,
                "imported_strain": 5.2,
                "hr_zone_seconds": { "z1": 168, "z2": 67, "z3": 0, "z4": 0, "z5": 0 },
                "confidence": 0.7
            }
        ]
    })
}

#[test]
fn whoop_history_batch_imports_and_is_idempotent() {
    let tempdir = tempfile::tempdir().unwrap();
    let db = tempdir.path().join("goose.sqlite");
    let db_path = db.display().to_string();

    let response = request(serde_json::json!({
        "schema": "goose.bridge.request.v1",
        "request_id": "whoop-import-1",
        "method": "import.whoop_history_batch",
        "args": sample_batch_args(&db_path)
    }));
    assert!(response.ok, "{:?}", response.error);
    let result = response.result.unwrap();
    assert_eq!(result["schema"], "murmur.import-result.v1");
    assert_eq!(result["recovery_written"], 1);
    assert_eq!(result["activity_written"], 1);
    assert_eq!(result["sleep_inserted"], 1);
    assert_eq!(result["workouts_inserted"], 1);
    assert_eq!(result["sleep_conflicts"], 0);
    assert_eq!(result["workouts_conflicts"], 0);

    // Re-import identico: daily upsert non cambia nulla, sleep/workout unchanged.
    let repeat = request(serde_json::json!({
        "schema": "goose.bridge.request.v1",
        "request_id": "whoop-import-2",
        "method": "import.whoop_history_batch",
        "args": sample_batch_args(&db_path)
    }));
    assert!(repeat.ok, "{:?}", repeat.error);
    let result = repeat.result.unwrap();
    assert_eq!(result["sleep_inserted"], 0);
    assert_eq!(result["sleep_unchanged"], 1);
    assert_eq!(result["workouts_inserted"], 0);
    assert_eq!(result["workouts_unchanged"], 1);
    assert_eq!(result["sleep_conflicts"], 0);
    assert_eq!(result["workouts_conflicts"], 0);
}

#[test]
fn whoop_history_batch_readable_via_daily_metrics() {
    let tempdir = tempfile::tempdir().unwrap();
    let db = tempdir.path().join("goose.sqlite");
    let db_path = db.display().to_string();

    let response = request(serde_json::json!({
        "schema": "goose.bridge.request.v1",
        "request_id": "whoop-import-read-1",
        "method": "import.whoop_history_batch",
        "args": sample_batch_args(&db_path)
    }));
    assert!(response.ok, "{:?}", response.error);

    let recovery = request(serde_json::json!({
        "schema": "goose.bridge.request.v1",
        "request_id": "whoop-import-read-2",
        "method": "metrics.daily_recovery_metrics",
        "args": {
            "database_path": db_path,
            "start_time_unix_ms": 0i64,
            "end_time_unix_ms": 4102444800000i64
        }
    }));
    assert!(recovery.ok, "{:?}", recovery.error);
    let result = recovery.result.unwrap();
    let rows = result["metrics"]
        .as_array()
        .or_else(|| result.as_array())
        .cloned()
        .unwrap_or_default();
    assert_eq!(rows.len(), 1, "unexpected read response: {result}");
    assert_eq!(rows[0]["date_key"], "2024-11-29");
    assert_eq!(rows[0]["source_kind"], "device_sensor");
    assert_eq!(rows[0]["hrv_rmssd_ms"], 60.0);
    let inputs: serde_json::Value =
        serde_json::from_str(rows[0]["inputs_json"].as_str().unwrap()).unwrap();
    assert_eq!(inputs["imported_score_0_to_100"], 53.0);
}

#[test]
fn whoop_history_batch_conflict_does_not_abort() {
    let tempdir = tempfile::tempdir().unwrap();
    let db = tempdir.path().join("goose.sqlite");
    let db_path = db.display().to_string();

    let first = request(serde_json::json!({
        "schema": "goose.bridge.request.v1",
        "request_id": "whoop-import-conflict-1",
        "method": "import.whoop_history_batch",
        "args": sample_batch_args(&db_path)
    }));
    assert!(first.ok, "{:?}", first.error);

    // Stesso import_key, end diverso: deve contare conflict e NON fallire.
    let mut args = sample_batch_args(&db_path);
    args["sleep"][0]["end_time_unix_ms"] = serde_json::json!(1732859033000i64);
    args["recovery"] = serde_json::json!([]);
    args["activity"] = serde_json::json!([]);
    args["workouts"] = serde_json::json!([]);
    let conflicted = request(serde_json::json!({
        "schema": "goose.bridge.request.v1",
        "request_id": "whoop-import-conflict-2",
        "method": "import.whoop_history_batch",
        "args": args
    }));
    assert!(conflicted.ok, "{:?}", conflicted.error);
    let result = conflicted.result.unwrap();
    assert_eq!(result["sleep_conflicts"], 1);
    assert_eq!(result["sleep_inserted"], 0);
}

#[test]
fn whoop_history_batch_rejects_forbidden_activity_type() {
    let tempdir = tempfile::tempdir().unwrap();
    let db = tempdir.path().join("goose.sqlite");
    let db_path = db.display().to_string();

    let mut args = sample_batch_args(&db_path);
    args["recovery"] = serde_json::json!([]);
    args["activity"] = serde_json::json!([]);
    args["sleep"] = serde_json::json!([]);
    args["workouts"][0]["activity_type"] = serde_json::json!("golf");

    let response = request(serde_json::json!({
        "schema": "goose.bridge.request.v1",
        "request_id": "whoop-import-forbidden-1",
        "method": "import.whoop_history_batch",
        "args": args
    }));
    assert!(!response.ok, "golf is not whitelisted and must fail");
}
