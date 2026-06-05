import Foundation

// Indice precalcolato dei dati storici importati: costruito UNA volta in
// background quando i report si caricano, consultato come lookup O(1) dal
// render path. Senza, ogni render rifaceva filter+sort+JSON parse su ~500
// righe, saturando il main thread.

struct HistoricalMetricValue {
  let value: Double
  let sourceKind: String
  let confidence: Double
  let endTimeUnixMS: Int64
}

struct HistoricalSleepNight {
  let startMS: Int64
  let endMS: Int64
  let durationMS: Int64
  let performancePercent: Double?
  let lightMinutes: Double?
  let deepMinutes: Double?
  let remMinutes: Double?
  let awakeMinutes: Double?
  // Stage-less sessions (band window detection) report a single "asleep" total.
  let asleepStageMinutes: Double?

  var asleepMinutes: Double {
    let staged = (lightMinutes ?? 0) + (deepMinutes ?? 0) + (remMinutes ?? 0)
    if staged > 0 {
      return staged
    }
    return asleepStageMinutes ?? 0
  }
}

struct HistoricalDayIndex {
  let recoveryValuesByDateKey: [String: [String: HistoricalMetricValue]]
  let activityValuesByDateKey: [String: [String: HistoricalMetricValue]]
  let recoveryScoreByDateKey: [String: Double]
  let strainByDateKey: [String: Double]
  let sleepNightByDateKey: [String: HistoricalSleepNight]

  static let empty = HistoricalDayIndex(
    recoveryValuesByDateKey: [:],
    activityValuesByDateKey: [:],
    recoveryScoreByDateKey: [:],
    strainByDateKey: [:],
    sleepNightByDateKey: [:]
  )
}

extension HealthDataStore {
  nonisolated static let historicalRecoveryValueKeys = [
    "resting_hr_bpm",
    "hrv_rmssd_ms",
    "respiratory_rate_rpm",
    "oxygen_saturation_percent",
    "skin_temperature_delta_c",
  ]

  nonisolated static let historicalActivityValueKeys = [
    "total_kcal",
    "active_kcal",
    "resting_kcal",
    "steps",
    "average_cadence_spm",
  ]

  nonisolated static func buildHistoricalDayIndex(
    recoveryRows: [[String: Any]],
    activityRows: [[String: Any]],
    sleepSessions: [[String: Any]]
  ) -> HistoricalDayIndex {
    var recoveryValues: [String: [String: HistoricalMetricValue]] = [:]
    var recoveryScores: [String: Double] = [:]
    for row in recoveryRows {
      guard let dateKey = row["date_key"] as? String else {
        continue
      }
      let sourceKind = row["source_kind"] as? String ?? ""
      let confidence = doubleValue(row["confidence"]) ?? 0
      let endMS = int64Value(row["end_time_unix_ms"]) ?? 0
      if sourceKind == "device_sensor" {
        // Stessa regola di dailyRecoveryMetricsWithValue + preferredDailyRecoveryMetric.
        for key in historicalRecoveryValueKeys {
          guard let value = doubleValue(row[key]) else {
            continue
          }
          let candidate = HistoricalMetricValue(
            value: value,
            sourceKind: sourceKind,
            confidence: confidence,
            endTimeUnixMS: endMS
          )
          if let existing = recoveryValues[dateKey]?[key],
             !historicalMetricValue(candidate, isBetterThan: existing) {
            continue
          }
          recoveryValues[dateKey, default: [:]][key] = candidate
        }
      }
      if let inputs = jsonObject(fromJSONString: row["inputs_json"]),
         let score = doubleValue(inputs["imported_score_0_to_100"]) {
        recoveryScores[dateKey] = score
      }
    }

    var activityValues: [String: [String: HistoricalMetricValue]] = [:]
    var strainScores: [String: Double] = [:]
    for row in activityRows {
      guard let dateKey = row["date_key"] as? String else {
        continue
      }
      let sourceKind = row["source_kind"] as? String ?? ""
      let confidence = doubleValue(row["confidence"]) ?? 0
      let endMS = int64Value(row["end_time_unix_ms"]) ?? 0
      for key in historicalActivityValueKeys {
        guard let value = doubleValue(row[key]) else {
          continue
        }
        let candidate = HistoricalMetricValue(
          value: value,
          sourceKind: sourceKind,
          confidence: confidence,
          endTimeUnixMS: endMS
        )
        if let existing = activityValues[dateKey]?[key],
           !historicalMetricValue(candidate, isBetterThan: existing) {
          continue
        }
        activityValues[dateKey, default: [:]][key] = candidate
      }
      if let inputs = jsonObject(fromJSONString: row["inputs_json"]),
         let strain = doubleValue(inputs["imported_strain"]) {
        strainScores[dateKey] = strain
      }
    }

    let dateKeyFormatter = DateFormatter()
    dateKeyFormatter.locale = Locale(identifier: "en_US_POSIX")
    dateKeyFormatter.dateFormat = "yyyy-MM-dd"

    var nights: [String: HistoricalSleepNight] = [:]
    for session in sleepSessions {
      guard let startMS = int64Value(session["start_time_unix_ms"]),
            let endMS = int64Value(session["end_time_unix_ms"]) else {
        continue
      }
      let provenance = jsonObject(fromJSONString: session["provenance_json"])
      if (provenance?["is_nap"] as? Bool) == true {
        continue
      }
      let durationMS = int64Value(session["duration_ms"]) ?? max(0, endMS - startMS)
      let summary = jsonObject(fromJSONString: session["stage_summary_json"])
      let minutes = summary?["minutes_by_stage"] as? [String: Any] ?? [:]
      let night = HistoricalSleepNight(
        startMS: startMS,
        endMS: endMS,
        durationMS: durationMS,
        performancePercent: doubleValue(provenance?["imported_performance_percent"]),
        lightMinutes: doubleValue(minutes["light"]),
        deepMinutes: doubleValue(minutes["deep"]),
        remMinutes: doubleValue(minutes["rem"]),
        awakeMinutes: doubleValue(minutes["awake"]),
        asleepStageMinutes: doubleValue(minutes["asleep"])
      )
      // La notte appartiene al giorno del risveglio; in caso di doppioni vince la piu' lunga.
      let dateKey = dateKeyFormatter.string(from: Date(timeIntervalSince1970: Double(endMS) / 1000))
      if let existing = nights[dateKey], existing.durationMS >= night.durationMS {
        continue
      }
      nights[dateKey] = night
    }

    return HistoricalDayIndex(
      recoveryValuesByDateKey: recoveryValues,
      activityValuesByDateKey: activityValues,
      recoveryScoreByDateKey: recoveryScores,
      strainByDateKey: strainScores,
      sleepNightByDateKey: nights
    )
  }

  nonisolated static func historicalMetricValue(
    _ lhs: HistoricalMetricValue,
    isBetterThan rhs: HistoricalMetricValue
  ) -> Bool {
    if lhs.confidence != rhs.confidence {
      return lhs.confidence > rhs.confidence
    }
    return lhs.endTimeUnixMS > rhs.endTimeUnixMS
  }

  // Mini-dizionario compatibile con i consumer esistenti dei "preferred metric"
  // (numberText su valueKey, dailyRecoveryMetricSource legge source_kind,
  // dailyRecoveryMetricStatus legge confidence).
  nonisolated static func historicalMetricRow(
    dateKey: String,
    valueKey: String,
    entry: HistoricalMetricValue
  ) -> [String: Any] {
    [
      "date_key": dateKey,
      valueKey: valueKey == "steps" ? Int(entry.value) as Any : entry.value as Any,
      "source_kind": entry.sourceKind,
      "confidence": entry.confidence,
      "end_time_unix_ms": entry.endTimeUnixMS,
    ]
  }
}
