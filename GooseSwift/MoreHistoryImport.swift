import Foundation

// Import dello storico (murmur-history.json) generato dal converter desktop.
// Il file arriva in Documents/GooseSwift/Import/ via Files/USB; ogni chunk viene
// passato al bridge Rust "import.whoop_history_batch" che valida e scrive nel DB
// in modo idempotente. Il file consumato viene rinominato .done.
extension MoreDataStore {
  static let historyImportSchema = "murmur.history.v1"
  static let historyImportSections = ["recovery", "activity", "sleep", "workouts"]
  static let historyImportChunkSize = 200

  nonisolated static func historyImportDirectoryURL() -> URL {
    let documents = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
    return documents
      .appendingPathComponent("GooseSwift", isDirectory: true)
      .appendingPathComponent("Import", isDirectory: true)
  }

  nonisolated static func discoverHistoryImportFile() -> URL? {
    let directory = historyImportDirectoryURL()
    try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    let contents = (try? FileManager.default.contentsOfDirectory(
      at: directory,
      includingPropertiesForKeys: nil
    )) ?? []
    return contents
      .filter { $0.pathExtension.lowercased() == "json" }
      .sorted { $0.lastPathComponent < $1.lastPathComponent }
      .first
  }

  func runHistoryImport() {
    guard !historyImportInProgress else {
      return
    }
    guard let fileURL = Self.discoverHistoryImportFile() else {
      historyImportStatus = "Nessun file .json in Documents/GooseSwift/Import"
      historyImportStatusKind = .pending
      return
    }

    historyImportInProgress = true
    historyImportStatus = "Importing \(fileURL.lastPathComponent)..."
    historyImportStatusKind = .pending
    let databasePath = self.databasePath

    DispatchQueue.global(qos: .userInitiated).async {
      let outcome = Self.performHistoryImport(fileURL: fileURL, databasePath: databasePath)
      DispatchQueue.main.async {
        self.historyImportInProgress = false
        self.historyImportStatus = outcome.summary
        self.historyImportStatusKind = outcome.failed ? .blocked : .ready
        if !outcome.failed {
          // Le schermate salute devono ricaricare i report dal DB appena importato.
          NotificationCenter.default.post(
            name: HealthDataStore.historyImportDidCompleteNotification,
            object: nil
          )
        }
      }
    }
  }

  nonisolated static func performHistoryImport(
    fileURL: URL,
    databasePath: String
  ) -> (summary: String, failed: Bool) {
    let payload: [String: Any]
    do {
      let data = try Data(contentsOf: fileURL)
      guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
        return ("File malformato: non è un oggetto JSON", true)
      }
      payload = object
    } catch {
      return ("Lettura file fallita: \(error.localizedDescription)", true)
    }

    guard payload["schema"] as? String == historyImportSchema else {
      return ("Schema non supportato: \(payload["schema"] ?? "assente")", true)
    }

    let bridge = GooseRustBridge()
    var totals: [String: Int] = [:]
    var chunkErrors: [String] = []

    for section in historyImportSections {
      let entries = payload[section] as? [[String: Any]] ?? []
      var index = 0
      while index < entries.count {
        let upper = min(index + historyImportChunkSize, entries.count)
        let chunk = Array(entries[index..<upper])
        index = upper
        do {
          let result = try bridge.request(
            method: "import.whoop_history_batch",
            args: [
              "database_path": databasePath,
              section: chunk,
            ]
          )
          for (key, value) in result {
            if let count = value as? Int {
              totals[key, default: 0] += count
            }
          }
        } catch {
          chunkErrors.append("\(section)[\(index)]: \(error)")
          if chunkErrors.count >= 3 {
            break
          }
        }
      }
    }

    if chunkErrors.isEmpty {
      let doneURL = fileURL.appendingPathExtension("done")
      try? FileManager.default.removeItem(at: doneURL)
      try? FileManager.default.moveItem(at: fileURL, to: doneURL)
    }

    let written = (totals["recovery_written"] ?? 0) + (totals["activity_written"] ?? 0)
      + (totals["sleep_inserted"] ?? 0) + (totals["workouts_inserted"] ?? 0)
    let unchanged = (totals["recovery_unchanged"] ?? 0) + (totals["activity_unchanged"] ?? 0)
      + (totals["sleep_unchanged"] ?? 0) + (totals["workouts_unchanged"] ?? 0)
    let conflicts = (totals["sleep_conflicts"] ?? 0) + (totals["workouts_conflicts"] ?? 0)

    var parts = ["Scritti \(written), invariati \(unchanged)"]
    if conflicts > 0 {
      parts.append("conflitti \(conflicts)")
    }
    if !chunkErrors.isEmpty {
      parts.append("ERRORI: \(chunkErrors.joined(separator: " | "))")
      return (parts.joined(separator: " | "), true)
    }
    return (parts.joined(separator: " | "), false)
  }
}
