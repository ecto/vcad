import Foundation

// OpenAI-compatible Chat Completions SSE, as served by frog api (and by a
// local `tang-llm serve`). Text arrives as `choices[0].delta.content`; tool
// calls stream as `delta.tool_calls[]` fragments keyed by `index`, with the
// id and name on the first fragment and `function.arguments` split across
// many. The stream ends with a `usage` chunk and `data: [DONE]`.

/// One parsed SSE frame.
enum FrogStreamEvent: Equatable {
    case text(String)
    case toolCall(index: Int, id: String?, name: String?, arguments: String)
    case finish(String)
    case usage(prompt: Int, completion: Int)
    case done
    /// An in-stream `{"error": {...}}` frame.
    case error(String)
}

/// Line-oriented SSE parser. Feed raw lines (without the trailing newline);
/// comments, blank lines and non-`data:` fields yield nothing.
struct FrogSSEParser {
    /// Events carried by one line (a chunk can hold text and tool calls).
    func parse(line: String) -> [FrogStreamEvent] {
        let trimmed = line.trimmingCharacters(in: CharacterSet(charactersIn: "\r"))
        guard trimmed.hasPrefix("data:") else { return [] }
        let payload = trimmed.dropFirst(5).trimmingCharacters(in: .whitespaces)
        if payload == "[DONE]" { return [.done] }
        guard let data = payload.data(using: .utf8),
              let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else { return [] }
        if let err = obj["error"] as? [String: Any] {
            return [.error(err["message"] as? String ?? "stream error")]
        }
        var out: [FrogStreamEvent] = []
        for choice in obj["choices"] as? [[String: Any]] ?? [] {
            let delta = choice["delta"] as? [String: Any] ?? [:]
            if let text = delta["content"] as? String, !text.isEmpty { out.append(.text(text)) }
            for call in delta["tool_calls"] as? [[String: Any]] ?? [] {
                let fn = call["function"] as? [String: Any] ?? [:]
                out.append(.toolCall(index: (call["index"] as? NSNumber)?.intValue ?? 0,
                                     id: call["id"] as? String,
                                     name: fn["name"] as? String,
                                     arguments: fn["arguments"] as? String ?? ""))
            }
            if let reason = choice["finish_reason"] as? String { out.append(.finish(reason)) }
        }
        if let usage = obj["usage"] as? [String: Any] {
            out.append(.usage(prompt: (usage["prompt_tokens"] as? NSNumber)?.intValue ?? 0,
                              completion: (usage["completion_tokens"] as? NSNumber)?.intValue ?? 0))
        }
        return out
    }
}

/// A complete tool call assembled from stream fragments.
struct FrogToolCall: Equatable {
    var id: String
    var name: String
    var arguments: String
}

/// Folds stream events into one assistant message.
struct FrogTurnAccumulator {
    private(set) var text = ""
    private var calls: [Int: FrogToolCall] = [:]
    private(set) var finishReason: String?
    private(set) var done = false
    private(set) var streamError: String?

    var toolCalls: [FrogToolCall] {
        calls.keys.sorted().compactMap { calls[$0] }.filter { !$0.name.isEmpty }.enumerated().map { i, c in
            var c = c
            if c.id.isEmpty { c.id = "call_\(i)" }
            return c
        }
    }

    mutating func apply(_ event: FrogStreamEvent) {
        switch event {
        case .text(let t): text += t
        case let .toolCall(index, id, name, args):
            var call = calls[index] ?? FrogToolCall(id: "", name: "", arguments: "")
            if let id, !id.isEmpty { call.id = id }
            if let name, !name.isEmpty { call.name += name }
            call.arguments += args
            calls[index] = call
        case .finish(let r): finishReason = r
        case .usage: break
        case .done: done = true
        case .error(let m): streamError = m
        }
    }

    /// The assistant message to append to the conversation.
    var assistantMessage: [String: Any] {
        var msg: [String: Any] = ["role": "assistant", "content": text]
        let calls = toolCalls
        if !calls.isEmpty {
            msg["tool_calls"] = calls.map { c in
                ["id": c.id, "type": "function",
                 "function": ["name": c.name, "arguments": c.arguments.isEmpty ? "{}" : c.arguments]] as [String: Any]
            }
        }
        return msg
    }
}
