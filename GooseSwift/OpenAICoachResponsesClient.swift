import Foundation

struct OpenAICoachToolCall: Equatable {
  let id: String
  let callID: String
  let name: String
  var arguments: String
}

struct OpenAIResponseStreamEvent {
  let type: String
  let payload: [String: Any]
}

enum OpenAIResponsesError: Error, LocalizedError {
  case missingOAuthSession
  case missingAccountID
  case invalidURL
  case invalidRequestBody
  case invalidResponse
  case httpStatus(Int, String)
  case api(String)

  var errorDescription: String? {
    switch self {
    case .missingOAuthSession:
      return "Sign in first."
    case .missingAccountID:
      return "Signed-in auth did not include an account id."
    case .invalidURL:
      return "The Coach Responses URL is invalid."
    case .invalidRequestBody:
      return "The Coach request could not be encoded."
    case .invalidResponse:
      return "Coach returned an invalid streaming response."
    case .httpStatus(let status, let body):
      return body.isEmpty ? "Coach request failed with HTTP \(status)." : "Coach request failed with HTTP \(status): \(body)"
    case .api(let message):
      return message
    }
  }
}

enum OpenAICoachRequestFactory {
  enum ToolMode {
    case required
    case auto
    case none
  }

  static func userInput(_ prompt: String) -> [[String: Any]] {
    [
      [
        "role": "user",
        "content": [
          [
            "type": "input_text",
            "text": prompt,
          ],
        ],
      ],
    ]
  }

  static func finalAnswerInput(originalPrompt: String) -> [String: Any] {
    [
      "role": "user",
      "content": [
        [
          "type": "input_text",
          "text": "Use the tool outputs above to answer this original Coach question now. Do not request more tools.\n\nOriginal question:\n\(originalPrompt)",
        ],
      ],
    ]
  }

  static func makeRequest(
    input: Any,
    toolMode: ToolMode,
    modelPreset: CoachModelPreset
  ) -> [String: Any] {
    var request: [String: Any] = [
      "model": modelPreset.modelID,
      "instructions": instructions,
      "input": input,
      "stream": true,
      "store": false,
      "reasoning": [
        "effort": modelPreset.effort,
      ],
      "text": [
        "verbosity": modelPreset.effort,
      ],
    ]
    switch toolMode {
    case .required:
      request["tools"] = tools
      request["parallel_tool_calls"] = false
      request["tool_choice"] = "required"
    case .auto:
      request["tools"] = tools
      request["parallel_tool_calls"] = false
      request["tool_choice"] = "auto"
    case .none:
      break
    }
    return request
  }

  private static let instructions = """
  You are Murmur Coach inside a user-owned WHOOP companion app. Use the available Murmur tools before making claims about health, activity, capture coverage, or device state. Cite tool names inline for metric claims, keep coaching practical, and say when data is missing or stale. Do not diagnose, prescribe, or infer medical conditions. Prefer one concrete next action when the local data is incomplete.
  """

  private static let emptyParameters: [String: Any] = [
    "type": "object",
    "properties": [:],
    "required": [],
    "additionalProperties": false,
  ]

  private static let tools: [[String: Any]] = [
    [
      "type": "function",
      "name": "load_stats",
      "description": "Load the current local Murmur metric snapshot, readiness status, score summaries, live heart-rate summary, and provenance.",
      "parameters": emptyParameters,
      "strict": true,
    ],
    [
      "type": "function",
      "name": "get_activities",
      "description": "Load the current manual activity, activity detection, movement packet, persistence, and route summaries.",
      "parameters": emptyParameters,
      "strict": true,
    ],
    [
      "type": "function",
      "name": "get_capture_sessions",
      "description": "Load local capture, packet import, Rust core/parser status, last parsed frame, and device evidence coverage.",
      "parameters": emptyParameters,
      "strict": true,
    ],
    [
      "type": "function",
      "name": "get_data_gaps",
      "description": "Load the concrete data gaps and next actions that should block or qualify Coach recommendations.",
      "parameters": emptyParameters,
      "strict": true,
    ],
  ]
}

struct OpenAIResponsesClient {
  private let endpoint = URL(string: "https://chatgpt.com/backend-api/codex/responses")

  func stream(
    auth: CodexStoredChatGPTAuth,
    body: [String: Any],
    onEvent: @MainActor @escaping (OpenAIResponseStreamEvent) throws -> Void
  ) async throws {
    guard let endpoint else {
      throw OpenAIResponsesError.invalidURL
    }
    let accountID = auth.accountID ?? CodexChatGPTTokenClaims.accountID(in: auth.accessToken)
    guard let accountID, !accountID.isEmpty else {
      throw OpenAIResponsesError.missingAccountID
    }
    guard JSONSerialization.isValidJSONObject(body) else {
      throw OpenAIResponsesError.invalidRequestBody
    }
    let bodyData = try JSONSerialization.data(withJSONObject: body, options: [])

    var request = URLRequest(url: endpoint)
    request.httpMethod = "POST"
    request.setValue("Bearer \(auth.accessToken)", forHTTPHeaderField: "Authorization")
    request.setValue(accountID, forHTTPHeaderField: "ChatGPT-Account-Id")
    request.setValue("application/json", forHTTPHeaderField: "Content-Type")
    request.setValue("text/event-stream", forHTTPHeaderField: "Accept")
    request.setValue("goose-swift", forHTTPHeaderField: "originator")
    request.httpBody = bodyData
    request.timeoutInterval = 180

    let (bytes, response) = try await URLSession.shared.bytes(for: request)
    guard let httpResponse = response as? HTTPURLResponse else {
      throw OpenAIResponsesError.invalidResponse
    }
    guard (200..<300).contains(httpResponse.statusCode) else {
      let body = try await readErrorBody(from: bytes)
      throw OpenAIResponsesError.httpStatus(httpResponse.statusCode, body)
    }

    var dataLines: [String] = []
    for try await line in bytes.lines {
      try Task.checkCancellation()
      let trimmedLine = line.trimmingCharacters(in: .whitespacesAndNewlines)
      if trimmedLine.isEmpty {
        try await process(dataLines: dataLines, onEvent: onEvent)
        dataLines.removeAll()
      } else if trimmedLine.hasPrefix("data:") {
        let value = String(trimmedLine.dropFirst(5)).trimmingCharacters(in: .whitespacesAndNewlines)
        dataLines.append(value)
      }
    }
    try await process(dataLines: dataLines, onEvent: onEvent)
  }

  private func process(
    dataLines: [String],
    onEvent: @MainActor @escaping (OpenAIResponseStreamEvent) throws -> Void
  ) async throws {
    guard !dataLines.isEmpty else {
      return
    }

    for dataText in dataLines.flatMap(Self.jsonPayloads(from:)) {
      guard dataText != "[DONE]",
            let data = dataText.data(using: .utf8),
            let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
            let type = object["type"] as? String else {
        continue
      }
      try await onEvent(OpenAIResponseStreamEvent(type: type, payload: object))
    }
  }

  private func readErrorBody(from bytes: URLSession.AsyncBytes) async throws -> String {
    var lines: [String] = []
    for try await line in bytes.lines {
      lines.append(line)
      if lines.joined().count > 4000 {
        break
      }
    }
    return lines.joined(separator: "\n")
  }

  private static func jsonPayloads(from dataLine: String) -> [String] {
    dataLine
      .split(whereSeparator: \.isNewline)
      .map { String($0).trimmingCharacters(in: .whitespacesAndNewlines) }
      .filter { !$0.isEmpty }
  }
}
