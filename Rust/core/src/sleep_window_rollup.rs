//! Sleep window detection from band-derived decoded frames.
//!
//! Ports the gravity-stillness detection validated by the reference Gen4
//! pipeline: deltas between consecutive gravity vectors below a stillness
//! threshold, smoothed over a rolling window, become sleep runs; gaps break
//! runs and short runs merge into their neighbours. The longest sleep run
//! above a minimum duration is the night's main sleep window. Window metrics
//! (HR min/avg/max, rolling RMSSD HRV) come from the same normal-history
//! frames, so a band historical sync is the only input this needs.

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    GooseResult,
    protocol::{DataPacketBodySummary, ParsedPayload},
    store::{ExternalSleepSessionInput, GooseStore},
};

pub const SLEEP_WINDOW_ROLLUP_REPORT_SCHEMA: &str = "goose.sleep-window-rollup-report.v1";
pub const GOOSE_SLEEP_WINDOW_BAND_SENSOR_V0_ID: &str = "goose.sleep_window.band_sensor.v0";
pub const GOOSE_SLEEP_WINDOW_BAND_SENSOR_V0_VERSION: &str = "0.1.0";
pub const BAND_SLEEP_SOURCE: &str = "band_sensor_detection";
pub const BAND_SLEEP_PLATFORM: &str = "local";

// Detection thresholds match the reference notebook analysis.
const STILL_DELTA_MAX_MILLI_G: f64 = 10.0; // 0.01 g between consecutive gravity samples
const ROLLING_WINDOW_SECONDS: i64 = 15 * 60;
const STILL_FRACTION_MIN: f64 = 0.70;
const MAX_GAP_SECONDS: i64 = 20 * 60; // data gaps larger than this break a run
const MERGE_BELOW_SECONDS: i64 = 15 * 60; // runs shorter than this merge into neighbours
const MIN_SLEEP_SECONDS: i64 = 60 * 60;
const RMSSD_ROLLING_WINDOW_RR: usize = 300;
const RMSSD_MIN_RR_FALLBACK: usize = 30;

#[derive(Debug, Clone, Copy)]
pub struct SleepWindowRollupOptions<'a> {
    pub date_key: &'a str,
    pub timezone: &'a str,
    pub start: &'a str,
    pub end: &'a str,
    pub write_session: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SleepWindowRollupReport {
    pub schema: String,
    pub generated_by: String,
    pub date_key: String,
    pub timezone: String,
    pub frame_count: usize,
    pub sample_count: usize,
    pub motion_sample_count: usize,
    pub detected: bool,
    pub sleep_id: Option<String>,
    pub start_time_unix_ms: Option<i64>,
    pub end_time_unix_ms: Option<i64>,
    pub duration_minutes: Option<f64>,
    pub hr_min_bpm: Option<f64>,
    pub hr_avg_bpm: Option<f64>,
    pub hr_max_bpm: Option<f64>,
    pub hrv_rmssd_ms: Option<f64>,
    pub rr_interval_count: usize,
    pub session_written: bool,
    pub issues: Vec<String>,
}

struct BandSample {
    unix_seconds: i64,
    bpm: Option<f64>,
    rr_ms: Vec<f64>,
    accel_milli_g: Option<[i32; 3]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunKind {
    Sleep,
    Active,
}

#[derive(Debug, Clone, Copy)]
struct Run {
    kind: RunKind,
    start_index: usize,
    end_index: usize,
}

pub fn rollup_sleep_window_for_store(
    store: &GooseStore,
    options: SleepWindowRollupOptions<'_>,
) -> GooseResult<SleepWindowRollupReport> {
    let mut issues = Vec::new();
    let rows = store.decoded_frames_between(options.start, options.end)?;
    let frame_count = rows.len();

    let mut samples = Vec::new();
    for row in &rows {
        let Ok(parsed) = serde_json::from_str::<ParsedPayload>(&row.parsed_payload_json) else {
            continue;
        };
        let ParsedPayload::DataPacket {
            timestamp_seconds: Some(timestamp_seconds),
            body_summary:
                Some(DataPacketBodySummary::NormalHistory {
                    marker_value,
                    rr_intervals_ms,
                    accel_milli_g,
                    ..
                }),
            ..
        } = parsed
        else {
            continue;
        };
        samples.push(BandSample {
            unix_seconds: i64::from(timestamp_seconds),
            bpm: marker_value.filter(|value| *value > 0).map(f64::from),
            rr_ms: rr_intervals_ms.iter().map(|value| f64::from(*value)).collect(),
            accel_milli_g,
        });
    }
    samples.sort_by_key(|sample| sample.unix_seconds);
    samples.dedup_by_key(|sample| sample.unix_seconds);
    let sample_count = samples.len();
    let motion_sample_count = samples
        .iter()
        .filter(|sample| sample.accel_milli_g.is_some())
        .count();

    let window = detect_sleep_window(&samples);
    if window.is_none() {
        issues.push("no_sleep_window_detected".to_string());
    }

    let mut report = SleepWindowRollupReport {
        schema: SLEEP_WINDOW_ROLLUP_REPORT_SCHEMA.to_string(),
        generated_by: "goose-core".to_string(),
        date_key: options.date_key.to_string(),
        timezone: options.timezone.to_string(),
        frame_count,
        sample_count,
        motion_sample_count,
        detected: false,
        sleep_id: None,
        start_time_unix_ms: None,
        end_time_unix_ms: None,
        duration_minutes: None,
        hr_min_bpm: None,
        hr_avg_bpm: None,
        hr_max_bpm: None,
        hrv_rmssd_ms: None,
        rr_interval_count: 0,
        session_written: false,
        issues,
    };

    let Some((window_start, window_end)) = window else {
        return Ok(report);
    };

    let in_window: Vec<&BandSample> = samples
        .iter()
        .filter(|sample| sample.unix_seconds >= window_start && sample.unix_seconds <= window_end)
        .collect();
    let bpm_values: Vec<f64> = in_window.iter().filter_map(|sample| sample.bpm).collect();
    let rr_values: Vec<f64> = in_window
        .iter()
        .flat_map(|sample| sample.rr_ms.iter().copied())
        .filter(|value| (300.0..=2000.0).contains(value))
        .collect();

    report.detected = true;
    report.start_time_unix_ms = Some(window_start * 1000);
    report.end_time_unix_ms = Some(window_end * 1000);
    report.duration_minutes = Some((window_end - window_start) as f64 / 60.0);
    report.hr_min_bpm = bpm_values.iter().copied().reduce(f64::min);
    report.hr_max_bpm = bpm_values.iter().copied().reduce(f64::max);
    report.hr_avg_bpm = if bpm_values.is_empty() {
        None
    } else {
        Some(bpm_values.iter().sum::<f64>() / bpm_values.len() as f64)
    };
    report.hrv_rmssd_ms = window_rmssd(&rr_values);
    report.rr_interval_count = rr_values.len();

    let sleep_id = format!("band-sleep-{}", options.date_key);
    report.sleep_id = Some(sleep_id.clone());

    if options.write_session {
        let duration_minutes = report.duration_minutes.unwrap_or(0.0);
        let stage_summary_json = json!({
            "minutes_by_stage": { "asleep": duration_minutes.round() },
        })
        .to_string();
        let provenance_json = json!({
            "algorithm": GOOSE_SLEEP_WINDOW_BAND_SENSOR_V0_ID,
            "algorithm_version": GOOSE_SLEEP_WINDOW_BAND_SENSOR_V0_VERSION,
            "source_kind": "device_sensor",
            "is_nap": false,
            "date_key": options.date_key,
            "timezone": options.timezone,
            "hr_min_bpm": report.hr_min_bpm,
            "hr_avg_bpm": report.hr_avg_bpm,
            "hr_max_bpm": report.hr_max_bpm,
            "hrv_rmssd_ms": report.hrv_rmssd_ms,
            "rr_interval_count": report.rr_interval_count,
            "sample_count": sample_count,
            "motion_sample_count": motion_sample_count,
        })
        .to_string();
        let input = ExternalSleepSessionInput {
            sleep_id: &sleep_id,
            source: BAND_SLEEP_SOURCE,
            platform: BAND_SLEEP_PLATFORM,
            platform_record_id: None,
            start_time_unix_ms: window_start * 1000,
            end_time_unix_ms: window_end * 1000,
            timezone: Some(options.timezone),
            stage_summary_json: &stage_summary_json,
            confidence: 0.75,
            provenance_json: &provenance_json,
        };
        // Re-running the rollup with more frames refines the same night:
        // replace our own previous detection instead of conflicting with it.
        // Validation errors on a fresh insert still propagate as errors.
        match store.external_sleep_session(&sleep_id)? {
            None => report.session_written = store.insert_external_sleep_session(input)?,
            Some(existing) if existing.source == BAND_SLEEP_SOURCE => {
                match store.insert_external_sleep_session(input.clone()) {
                    Ok(written) => report.session_written = written,
                    Err(_) => {
                        store.delete_external_sleep_session(&sleep_id)?;
                        report.session_written = store.insert_external_sleep_session(input)?;
                    }
                }
            }
            Some(_) => report
                .issues
                .push("sleep_id_conflict_with_foreign_session".to_string()),
        }
    }

    Ok(report)
}

/// Longest still run above the minimum duration, as (start, end) unix seconds.
fn detect_sleep_window(samples: &[BandSample]) -> Option<(i64, i64)> {
    if samples.len() < 2 {
        return None;
    }

    // Delta between consecutive gravity vectors; missing motion data counts as moving.
    let mut deltas = Vec::with_capacity(samples.len());
    deltas.push(f64::MAX);
    for window in samples.windows(2) {
        let delta = match (window[0].accel_milli_g, window[1].accel_milli_g) {
            (Some(a), Some(b)) => {
                let dx = f64::from(a[0] - b[0]);
                let dy = f64::from(a[1] - b[1]);
                let dz = f64::from(a[2] - b[2]);
                (dx * dx + dy * dy + dz * dz).sqrt()
            }
            _ => f64::MAX,
        };
        deltas.push(delta);
    }

    // Median sampling interval sizes the rolling window in sample counts.
    let mut intervals: Vec<i64> = samples
        .windows(2)
        .map(|window| window[1].unix_seconds - window[0].unix_seconds)
        .filter(|delta| (1..300).contains(delta))
        .collect();
    intervals.sort_unstable();
    let median_interval = intervals
        .get(intervals.len() / 2)
        .copied()
        .unwrap_or(60)
        .max(1);
    let window_size = ((ROLLING_WINDOW_SECONDS / median_interval) as usize).max(3);

    let total = deltas.len();
    let is_sleep: Vec<bool> = (0..total)
        .map(|index| {
            let half = window_size / 2;
            let start = index.saturating_sub(half);
            let end = (index + half + 1).min(total);
            let slice = &deltas[start..end];
            let still = slice
                .iter()
                .filter(|delta| **delta < STILL_DELTA_MAX_MILLI_G)
                .count();
            still as f64 / slice.len() as f64 >= STILL_FRACTION_MIN
        })
        .collect();

    // Build runs, breaking on class changes and data gaps.
    let mut runs = Vec::new();
    let mut run_start = 0usize;
    for index in 1..=total {
        let end_of_data = index == total;
        let class_change = !end_of_data && is_sleep[index] != is_sleep[run_start];
        let gap_break = !end_of_data
            && samples[index].unix_seconds - samples[index - 1].unix_seconds > MAX_GAP_SECONDS;
        if end_of_data || class_change || gap_break {
            runs.push(Run {
                kind: if is_sleep[run_start] {
                    RunKind::Sleep
                } else {
                    RunKind::Active
                },
                start_index: run_start,
                end_index: index - 1,
            });
            if !end_of_data {
                run_start = index;
            }
        }
    }

    let merged = merge_short_runs(samples, runs);

    merged
        .iter()
        .filter(|run| run.kind == RunKind::Sleep)
        .map(|run| {
            (
                samples[run.start_index].unix_seconds,
                samples[run.end_index].unix_seconds,
            )
        })
        .filter(|(start, end)| end - start >= MIN_SLEEP_SECONDS)
        .max_by_key(|(start, end)| end - start)
}

fn merge_short_runs(samples: &[BandSample], runs: Vec<Run>) -> Vec<Run> {
    if runs.is_empty() {
        return Vec::new();
    }

    let duration = |run: &Run| {
        samples[run.end_index].unix_seconds - samples[run.start_index].unix_seconds
    };

    let mut merged: Vec<Run> = Vec::new();
    let mut runs = runs;
    let mut index = 0usize;
    while index < runs.len() {
        let current = runs[index];
        if duration(&current) < MERGE_BELOW_SECONDS {
            if index > 0
                && index + 1 < runs.len()
                && runs[index - 1].kind == runs[index + 1].kind
                && !merged.is_empty()
            {
                let previous = merged.pop().expect("checked non-empty");
                merged.push(Run {
                    kind: previous.kind,
                    start_index: previous.start_index,
                    end_index: runs[index + 1].end_index,
                });
                index += 1; // the next run is absorbed too
            } else if index + 1 < runs.len() {
                runs[index + 1] = Run {
                    kind: runs[index + 1].kind,
                    start_index: current.start_index,
                    end_index: runs[index + 1].end_index,
                };
            } else if let Some(previous) = merged.pop() {
                merged.push(Run {
                    kind: previous.kind,
                    start_index: previous.start_index,
                    end_index: current.end_index,
                });
            }
        } else {
            merged.push(current);
        }
        index += 1;
    }

    merged
}

/// Average rolling RMSSD over 300-interval windows; falls back to a direct
/// RMSSD when the night has too few RR intervals for a rolling estimate.
fn window_rmssd(rr_values: &[f64]) -> Option<f64> {
    if rr_values.len() >= RMSSD_ROLLING_WINDOW_RR {
        let rolling: Vec<f64> = rr_values
            .windows(RMSSD_ROLLING_WINDOW_RR)
            .filter_map(rmssd)
            .collect();
        if rolling.is_empty() {
            return None;
        }
        return Some(rolling.iter().sum::<f64>() / rolling.len() as f64);
    }
    if rr_values.len() >= RMSSD_MIN_RR_FALLBACK {
        return rmssd(rr_values);
    }
    None
}

fn rmssd(window: &[f64]) -> Option<f64> {
    if window.len() < 2 {
        return None;
    }
    let squared_diffs: Vec<f64> = window
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).powi(2))
        .collect();
    Some((squared_diffs.iter().sum::<f64>() / squared_diffs.len() as f64).sqrt())
}
