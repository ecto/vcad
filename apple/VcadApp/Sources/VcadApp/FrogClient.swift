import Foundation
import Security

// Transport for the built-in agent: an OpenAI-compatible Chat Completions
// endpoint (tang cloud, or a local `tang-llm serve`). Tools always run here,
// client-side; the endpoint only streams text and tool calls back. Endpoint,
// model id and capabilities come from Settings (UserDefaults) with env
// overrides; the API key lives only in the Keychain (or the env).

/// Where and how the agent talks to its model.
struct FrogConfig: Equatable {
    var baseURL: String
    /// Model id sent as `model`; from config only — never a Swift literal.
    var model: String
    var apiKey: String?
    /// Endpoint accepts `image_url` content parts (a vision model). Off by
    /// default: the agent then works from text scene descriptions.
    var supportsImages: Bool

    static let baseKey = "vcad.agent.baseURL"
    static let modelKey = "vcad.agent.model"
    static let imagesKey = "vcad.agent.supportsImages"

    /// Settings, then env (`VCAD_ZEST_BASE`, `VCAD_ZEST_KEY`, `VCAD_ZEST_MODEL`,
    /// `VCAD_ZEST_IMAGES=1`) on top.
    static func load(defaults: UserDefaults = .standard,
                     env: [String: String] = ProcessInfo.processInfo.environment,
                     keychain: FrogKeychain = .shared) -> FrogConfig {
        func nonEmpty(_ s: String?) -> String? { s.flatMap { $0.isEmpty ? nil : $0 } }
        return FrogConfig(
            baseURL: nonEmpty(env["VCAD_ZEST_BASE"]) ?? nonEmpty(defaults.string(forKey: baseKey)) ?? "",
            model: nonEmpty(env["VCAD_ZEST_MODEL"]) ?? nonEmpty(defaults.string(forKey: modelKey)) ?? "",
            apiKey: nonEmpty(env["VCAD_ZEST_KEY"]) ?? keychain.read(),
            supportsImages: env["VCAD_ZEST_IMAGES"].map { $0 == "1" } ?? defaults.bool(forKey: imagesKey))
    }

    var completionsURL: URL? {
        guard !baseURL.isEmpty else { return nil }
        var base = baseURL
        while base.hasSuffix("/") { base.removeLast() }
        return URL(string: base + "/chat/completions")
    }
}

/// The API key in the login Keychain (generic password).
struct FrogKeychain {
    static let shared = FrogKeychain(service: "io.vcad.agent", account: "api-key")
    let service: String
    let account: String

    private var query: [String: Any] {
        [kSecClass as String: kSecClassGenericPassword,
         kSecAttrService as String: service,
         kSecAttrAccount as String: account]
    }

    func read() -> String? {
        var q = query
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var out: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess,
              let data = out as? Data, let key = String(data: data, encoding: .utf8), !key.isEmpty else { return nil }
        return key
    }

    @discardableResult
    func write(_ key: String?) -> Bool {
        SecItemDelete(query as CFDictionary)
        guard let key, !key.isEmpty else { return true }
        var q = query
        q[kSecValueData as String] = Data(key.utf8)
        return SecItemAdd(q as CFDictionary, nil) == errSecSuccess
    }
}

enum FrogError: LocalizedError, Equatable {
    case notConfigured(String)
    case unauthorized
    case outOfCredit
    case rateLimited(retryAfter: Double?)
    case unavailable(retryAfter: Double?)
    case http(Int, String)
    case transport(String)
    case stream(String)

    var errorDescription: String? {
        switch self {
        case .notConfigured(let what): return "Agent isn't set up: \(what). Open Settings ▸ Agent."
        case .unauthorized: return "tang cloud rejected the API key. Check it in Settings ▸ Agent."
        case .outOfCredit: return "Your tang cloud credit is used up."
        case .rateLimited: return "Too many requests — try again in a moment."
        case .unavailable(let after):
            return "tang cloud is unavailable right now\(after.map { " (retry in \(Int($0)) s)" } ?? "")."
        case .http(let code, let message): return "Agent service error \(code): \(message)"
        case .transport(let m): return "Network error: \(m)"
        case .stream(let m): return "The model stream failed: \(m)"
        }
    }

    /// Map a non-2xx response (`{"error": {"message", "type", "code"}}`).
    static func from(status: Int, body: Data, retryAfter: String?) -> FrogError {
        let obj = (try? JSONSerialization.jsonObject(with: body)) as? [String: Any]
        let message = (obj?["error"] as? [String: Any])?["message"] as? String
            ?? String(data: body.prefix(200), encoding: .utf8) ?? ""
        let after = retryAfter.flatMap(Double.init)
        switch status {
        case 401: return .unauthorized
        case 402: return .outOfCredit
        case 429: return .rateLimited(retryAfter: after)
        case 503: return .unavailable(retryAfter: after)
        default: return .http(status, message)
        }
    }
}

/// Sends one request and yields SSE lines. Mocked in tests.
protocol FrogTransport: Sendable {
    func lines(for request: URLRequest) async throws -> AsyncThrowingStream<String, Error>
}

/// URLSession streaming transport.
struct URLSessionFrogTransport: FrogTransport {
    func lines(for request: URLRequest) async throws -> AsyncThrowingStream<String, Error> {
        let bytes: URLSession.AsyncBytes
        let response: URLResponse
        do {
            (bytes, response) = try await URLSession.shared.bytes(for: request)
        } catch {
            throw FrogError.transport(error.localizedDescription)
        }
        let http = response as? HTTPURLResponse
        if let http, !(200..<300).contains(http.statusCode) {
            var body = Data()
            for try await b in bytes { body.append(b); if body.count > 8192 { break } }
            throw FrogError.from(status: http.statusCode, body: body,
                                 retryAfter: http.value(forHTTPHeaderField: "retry-after"))
        }
        return AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    for try await line in bytes.lines { continuation.yield(line) }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }
}

/// Builds Chat Completions requests for one conversation (main-actor: the
/// conversation is plain JSON dictionaries owned by the agent).
@MainActor
struct FrogClient {
    var config: FrogConfig
    var transport: FrogTransport = URLSessionFrogTransport()

    func request(messages: [[String: Any]], tools: [[String: Any]], cacheKey: String) throws -> URLRequest {
        guard let url = config.completionsURL else { throw FrogError.notConfigured("endpoint URL") }
        guard !config.model.isEmpty else { throw FrogError.notConfigured("model id") }
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.timeoutInterval = 300
        req.setValue("application/json", forHTTPHeaderField: "content-type")
        req.setValue("text/event-stream", forHTTPHeaderField: "accept")
        if let key = config.apiKey { req.setValue("Bearer \(key)", forHTTPHeaderField: "authorization") }
        var body: [String: Any] = [
            "model": config.model,
            "messages": messages,
            "stream": true,
            "stream_options": ["include_usage": true],
            "prompt_cache_key": cacheKey,
        ]
        if !tools.isEmpty { body["tools"] = tools; body["tool_choice"] = "auto" }
        req.httpBody = try JSONSerialization.data(withJSONObject: body)
        return req
    }

    /// Stream one completion, reporting events as they parse.
    func complete(messages: [[String: Any]], tools: [[String: Any]], cacheKey: String,
                  onEvent: (FrogStreamEvent) -> Void) async throws -> FrogTurnAccumulator {
        let req = try request(messages: messages, tools: tools, cacheKey: cacheKey)
        let parser = FrogSSEParser()
        var acc = FrogTurnAccumulator()
        for try await line in try await transport.lines(for: req) {
            try Task.checkCancellation()
            for event in parser.parse(line: line) {
                acc.apply(event)
                onEvent(event)
            }
            if acc.done { break }
        }
        if let err = acc.streamError { throw FrogError.stream(err) }
        return acc
    }
}
