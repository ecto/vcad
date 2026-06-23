import Foundation

// Transport for the AI intent bar. The native app talks to vcad's OWN backend
// (the same /api/chat the web app uses) — never directly to Anthropic — so the
// Claude key stays server-side and the user never pastes or manages one. The
// endpoint allows anonymous calls (IP rate-limited free tier), takes a custom
// systemPrompt, and streams the reply as SSE. We send the loon-authoring prompt
// with no tools, so the model streams back a loon program as plain text.

enum IntentError: LocalizedError {
    case transport(Error)
    case rateLimited
    case flagged(String?)
    case notConfigured
    case http(status: Int, message: String)
    case emptyResponse

    var errorDescription: String? {
        switch self {
        case .transport(let e):
            return "Network error: \(e.localizedDescription)"
        case .rateLimited:
            return "Daily free limit reached — try again later."
        case .flagged(let reason):
            return "That request was declined\(reason.map { ": \($0)" } ?? ".")"
        case .notConfigured:
            return "The vcad AI service is unavailable right now."
        case .http(let status, let message):
            return "Service error \(status): \(message)"
        case .emptyResponse:
            return "The model returned no geometry."
        }
    }
}

/// Stateless client for vcad's hosted AI endpoint. No credentials live here —
/// the backend owns the Anthropic key and the model choice.
struct VcadIntentClient {
    /// Production by default; override with `VCAD_API_BASE` (e.g. a localhost
    /// dev server) when the app is launched from a terminal.
    static var baseURL: String {
        if let env = ProcessInfo.processInfo.environment["VCAD_API_BASE"], !env.isEmpty {
            return env
        }
        return "https://vcad.io"
    }
    static var endpoint: URL { URL(string: "\(baseURL)/api/chat")! }

    /// Ask the backend to author a loon program for `user`, steered by `system`.
    /// Returns the assistant's text (a loon program). The reply is a short
    /// program, so we buffer the SSE response and collect its `text` frames
    /// rather than streaming incrementally.
    func complete(system: String, user: String) async throws -> String {
        var req = URLRequest(url: Self.endpoint)
        req.httpMethod = "POST"
        req.timeoutInterval = 120
        req.setValue("application/json", forHTTPHeaderField: "content-type")
        // Deliberately NO Authorization header — anonymous free tier; the key is
        // server-side. (A native URLSession isn't subject to CORS.)
        let body: [String: Any] = [
            "messages": [["role": "user", "content": user]],
            "systemPrompt": system,
        ]
        req.httpBody = try JSONSerialization.data(withJSONObject: body)

        let data: Data
        let http: HTTPURLResponse
        do {
            let (d, resp) = try await URLSession.shared.data(for: req)
            data = d
            http = resp as? HTTPURLResponse ?? HTTPURLResponse()
        } catch {
            throw IntentError.transport(error)
        }

        guard (200..<300).contains(http.statusCode) else {
            throw Self.mapError(status: http.statusCode, body: data)
        }

        let text = Self.collectText(fromSSE: data)
        guard !text.isEmpty else { throw IntentError.emptyResponse }
        return text
    }

    /// Concatenate the text of every `data: {"type":"text","text":…}` SSE frame.
    private static func collectText(fromSSE data: Data) -> String {
        guard let raw = String(data: data, encoding: .utf8) else { return "" }
        var out = ""
        for line in raw.split(separator: "\n", omittingEmptySubsequences: true) {
            guard line.hasPrefix("data: ") else { continue }
            let json = line.dropFirst("data: ".count)
            guard let d = json.data(using: .utf8),
                  let obj = try? JSONSerialization.jsonObject(with: d) as? [String: Any],
                  let type = obj["type"] as? String else { continue }
            if type == "text", let t = obj["text"] as? String { out += t }
        }
        return out.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// Map a non-2xx body to a specific error: 429/`anon_limit` (free pool
    /// spent), 400/`flagged` (safety), 503 (backend not configured).
    private static func mapError(status: Int, body: Data) -> IntentError {
        let obj = (try? JSONSerialization.jsonObject(with: body)) as? [String: Any]
        // vcad's own errors use a string `error` (e.g. "anon_limit", "flagged");
        // an upstream Anthropic error is passed through as `error: { message }`.
        let code = obj?["error"] as? String
        let nested = (obj?["error"] as? [String: Any])?["message"] as? String
        let reason = (obj?["reason"] as? String) ?? (obj?["message"] as? String) ?? nested
        if status == 429 || code == "anon_limit" { return .rateLimited }
        if status == 400, code == "flagged" { return .flagged(reason) }
        if status == 503 { return .notConfigured }
        let detail = nested ?? code ?? reason
            ?? (String(data: body, encoding: .utf8)?.prefix(160).description ?? "unknown")
        return .http(status: status, message: detail)
    }
}
