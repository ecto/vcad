import XCTest
@testable import VcadApp

/// Scripted SSE endpoint: each request gets the next script, which can read
/// the request body (to pick up ids the engine minted in earlier rounds).
final class MockFrogTransport: FrogTransport, @unchecked Sendable {
    typealias Script = @Sendable ([String: Any]) -> [String]
    private let lock = NSLock()
    private var scripts: [Script]
    private(set) var bodies: [[String: Any]] = []

    /// `GET /models` response (body, status); nil fails the request.
    var models: (Data, Int)?
    private(set) var plainRequests: [URLRequest] = []

    init(_ scripts: [Script], models: (Data, Int)? = nil) {
        self.scripts = scripts
        self.models = models
    }

    func data(for request: URLRequest) async throws -> (Data, Int) {
        lock.withLock { plainRequests.append(request) }
        guard let models else { throw FrogError.transport("no models response scripted") }
        return models
    }

    func lines(for request: URLRequest) async throws -> AsyncThrowingStream<String, Error> {
        let body = (try? JSONSerialization.jsonObject(with: request.httpBody ?? Data())) as? [String: Any] ?? [:]
        let script: Script? = lock.withLock {
            bodies.append(body)
            return scripts.isEmpty ? nil : scripts.removeFirst()
        }
        guard let script else { throw FrogError.http(500, "no script left") }
        let lines = script(body)
        return AsyncThrowingStream { c in
            for l in lines { c.yield(l) }
            c.finish()
        }
    }
}

/// SSE helpers shaped like frog api's output.
enum SSE {
    static func chunk(_ delta: [String: Any], finish: String? = nil) -> String {
        var choice: [String: Any] = ["index": 0, "delta": delta]
        choice["finish_reason"] = finish ?? NSNull()
        let obj: [String: Any] = ["id": "c", "object": "chat.completion.chunk", "choices": [choice]]
        return "data: " + String(decoding: try! JSONSerialization.data(withJSONObject: obj), as: UTF8.self)
    }
    /// A tool call streamed as a header fragment plus argument fragments.
    static func toolCall(id: String, name: String, args: String, pieces: Int = 3) -> [String] {
        var out = [chunk(["tool_calls": [["index": 0, "id": id, "type": "function",
                                          "function": ["name": name, "arguments": ""]]]])]
        let chars = Array(args)
        let step = max(1, chars.count / pieces + 1)
        var i = 0
        while i < chars.count {
            let frag = String(chars[i..<min(chars.count, i + step)])
            out.append(chunk(["tool_calls": [["index": 0, "function": ["arguments": frag]]]]))
            i += step
        }
        out.append("")
        out.append(chunk([:], finish: "tool_calls"))
        out.append("data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5}}")
        out.append("data: [DONE]")
        return out
    }
    static func text(_ parts: [String]) -> [String] {
        parts.map { chunk(["content": $0]) } + [chunk([:], finish: "stop"), "data: [DONE]"]
    }
    static func json(_ v: [String: Any]) -> String {
        String(decoding: try! JSONSerialization.data(withJSONObject: v), as: UTF8.self)
    }
}

/// The id the engine reported in the most recent tool result.
private func lastCreatedId(_ body: [String: Any]) -> String {
    let messages = body["messages"] as? [[String: Any]] ?? []
    for m in messages.reversed() where m["role"] as? String == "tool" {
        let content = m["content"] as? String ?? ""
        if let r = content.range(of: "created part ") {
            return String(content[r.upperBound...].prefix { $0 != ";" })
        }
    }
    return "missing"
}

@MainActor
final class FrogAgentTests: XCTestCase {
    private func config(images: Bool = false) -> FrogConfig {
        FrogConfig(baseURL: "http://127.0.0.1:9/v1", model: "test-model", apiKey: "test", supportsImages: images)
    }

    private func agent(_ transport: MockFrogTransport, engine: DocumentEngine, images: Bool = false,
                       snapshot: ((FrogView) async -> Data?)? = nil) -> FrogAgent {
        let executor = FrogToolExecutor(engine: engine, snapshot: snapshot)
        return FrogAgent(client: FrogClient(config: config(images: images), transport: transport),
                         manifest: .load(), executor: executor)
    }

    // MARK: - the 3-turn tool loop

    func testThreeRoundToolLoopCreatesAndMovesAPart() async throws {
        let engine = DocumentEngine(replicaId: 1)
        let transport = MockFrogTransport([
            { _ in SSE.toolCall(id: "call_a", name: "create",
                                args: SSE.json(["type": "cube", "name": "Block",
                                                "params": ["size": ["x": 10, "y": 20, "z": 30]]])) },
            { body in SSE.toolCall(id: "call_b", name: "translate",
                                   args: SSE.json(["child": lastCreatedId(body),
                                                   "offset": ["x": 5, "y": 0, "z": 7]])) },
            { _ in SSE.text(["Added a ", "block and moved it."]) },
        ])
        let a = agent(transport, engine: engine)
        var text = ""
        var finished: [FrogToolExecutor.Outcome] = []
        let mutated = try await a.send("make a block and move it") { event in
            switch event {
            case .text(let t): text += t
            case .toolFinished(_, let o): finished.append(o)
            case .toolStarted: break
            }
        }

        XCTAssertTrue(mutated)
        XCTAssertEqual(text, "Added a block and moved it.")
        XCTAssertEqual(finished.map(\.name), ["create", "translate"])
        XCTAssertTrue(finished.allSatisfy(\.ok), finished.map(\.content).joined(separator: " | "))

        // The engine document: one cube, sized and offset as the model asked.
        let state = try engine.state()
        XCTAssertEqual(state.partCount, 1)
        let nodes = state.document["nodes"] as? [String: Any] ?? [:]
        let ops = nodes.values.compactMap { ($0 as? [String: Any])?["op"] as? [String: Any] }
        let cube = try XCTUnwrap(ops.first { $0["type"] as? String == "Cube" })
        let size = cube["size"] as? [String: Any] ?? [:]
        XCTAssertEqual((size["z"] as? NSNumber)?.doubleValue, 30)
        let translate = try XCTUnwrap(ops.first { $0["type"] as? String == "Translate" })
        let offset = translate["offset"] as? [String: Any] ?? [:]
        XCTAssertEqual((offset["x"] as? NSNumber)?.doubleValue, 5)
        XCTAssertEqual((offset["z"] as? NSNumber)?.doubleValue, 7)

        // Wire contract: 3 requests, stable cache key, model from config,
        // streamed, tools + system prompt every time, tool results echoed.
        XCTAssertEqual(transport.bodies.count, 3)
        let keys = Set(transport.bodies.compactMap { $0["prompt_cache_key"] as? String })
        XCTAssertEqual(keys.count, 1)
        for body in transport.bodies {
            XCTAssertEqual(body["model"] as? String, "test-model")
            XCTAssertEqual(body["stream"] as? Bool, true)
            XCTAssertFalse((body["tools"] as? [Any] ?? []).isEmpty)
            XCTAssertEqual((body["messages"] as? [[String: Any]])?.first?["role"] as? String, "system")
        }
        let last = transport.bodies[2]["messages"] as? [[String: Any]] ?? []
        XCTAssertEqual(last.map { $0["role"] as? String ?? "" }, ["system", "user", "assistant", "tool", "assistant", "tool"])
        XCTAssertEqual(last[3]["tool_call_id"] as? String, "call_a")
        let calls = last[2]["tool_calls"] as? [[String: Any]] ?? []
        XCTAssertEqual((calls.first?["function"] as? [String: Any])?["name"] as? String, "create")
    }

    func testToolFailuresGoBackToTheModel() async throws {
        let engine = DocumentEngine(replicaId: 1)
        let transport = MockFrogTransport([
            { _ in SSE.toolCall(id: "c1", name: "translate", args: SSE.json(["child": "9:9", "offset": ["x": 1, "y": 0, "z": 0]])) },
            { _ in SSE.toolCall(id: "c2", name: "launch_rocket", args: "{}") },
            { _ in SSE.toolCall(id: "c3", name: "create", args: "{not json") },
            { _ in SSE.text(["I could not do that."]) },
        ])
        let mutated = try await agent(transport, engine: engine).send("move it") { _ in }
        XCTAssertFalse(mutated)
        let msgs = transport.bodies[3]["messages"] as? [[String: Any]] ?? []
        let results = msgs.filter { $0["role"] as? String == "tool" }.compactMap { $0["content"] as? String }
        XCTAssertEqual(results.count, 3)
        XCTAssertTrue(results[0].hasPrefix("error (unknown_id)"), results[0])
        XCTAssertTrue(results[1].contains("unsupported"), results[1])
        XCTAssertTrue(results[2].contains("not a JSON object"), results[2])
        XCTAssertEqual(try engine.state().partCount, 0)
    }

    func testReadToolsDescribeAndInspect() async throws {
        let engine = DocumentEngine(replicaId: 1)
        let created = try engine.applyTool(ToolCall(tool: "create", args: ["type": "cube", "params": ["size": ["x": 10, "y": 10, "z": 10]]]))
        let id = try XCTUnwrap(created.createdFeatureId)
        let ex = FrogToolExecutor(engine: engine)
        let scene = await ex.execute(FrogToolCall(id: "1", name: "describe_scene", arguments: ""))
        XCTAssertTrue(scene.ok); XCTAssertTrue(scene.content.contains(id), scene.content)
        let inspect = await ex.execute(FrogToolCall(id: "2", name: "inspect", arguments: SSE.json(["part_id": id])))
        XCTAssertTrue(inspect.content.contains("volume 1000.000"), inspect.content)
        XCTAssertNil(inspect.mutation)
    }

    // MARK: - vision capability

    func testVisionOnOffersRenderViewAndAttachesImage() async throws {
        let engine = DocumentEngine(replicaId: 1)
        let png = Data([0x89, 0x50, 0x4E, 0x47])
        let transport = MockFrogTransport([
            { _ in SSE.toolCall(id: "v1", name: "render_view", args: "{}") },
            { _ in SSE.text(["Looks good."]) },
        ])
        try await agent(transport, engine: engine, images: true, snapshot: { _ in png }).send("look") { _ in }
        let names = (transport.bodies[0]["tools"] as? [[String: Any]] ?? [])
            .compactMap { ($0["function"] as? [String: Any])?["name"] as? String }
        XCTAssertTrue(names.contains("render_view"))
        let msgs = transport.bodies[1]["messages"] as? [[String: Any]] ?? []
        let image = try XCTUnwrap(msgs.last?["content"] as? [[String: Any]])
        XCTAssertEqual(image.first?["type"] as? String, "image_url")
        let url = (image.first?["image_url"] as? [String: Any])?["url"] as? String
        XCTAssertEqual(url, "data:image/png;base64,\(png.base64EncodedString())")
    }

    func testVisionOffFallsBackToTextScene() async throws {
        let engine = DocumentEngine(replicaId: 1)
        try engine.applyTool(ToolCall(tool: "create", args: ["type": "sphere", "params": ["radius": 4]]))
        let transport = MockFrogTransport([
            { _ in SSE.toolCall(id: "v1", name: "render_view", args: "{}") },
            { _ in SSE.text(["ok"]) },
        ])
        var snapshotCalled = false
        try await agent(transport, engine: engine, images: false,
                        snapshot: { _ in snapshotCalled = true; return Data([1]) }).send("look") { _ in }
        let names = (transport.bodies[0]["tools"] as? [[String: Any]] ?? [])
            .compactMap { ($0["function"] as? [String: Any])?["name"] as? String }
        XCTAssertFalse(names.contains("render_view"))
        XCTAssertFalse(snapshotCalled)
        let msgs = transport.bodies[1]["messages"] as? [[String: Any]] ?? []
        XCTAssertFalse(msgs.contains { $0["content"] is [Any] }, "no image parts without the capability")
        let tool = try XCTUnwrap(msgs.last { $0["role"] as? String == "tool" }?["content"] as? String)
        XCTAssertTrue(tool.contains("text scene description"), tool)
        XCTAssertTrue(tool.contains("sphere"), tool)
    }

    // MARK: - prompt cache

    func testCacheablePrefixIsIdenticalAcrossSessions() async throws {
        func firstRequest() async throws -> (prefix: Data, key: String) {
            let transport = MockFrogTransport([{ _ in SSE.text(["hi"]) }])
            try await agent(transport, engine: DocumentEngine(replicaId: UInt64.random(in: 1...999)))
                .send("session-specific text \(UUID())") { _ in }
            let body = try XCTUnwrap(transport.bodies.first)
            let messages = try XCTUnwrap(body["messages"] as? [[String: Any]])
            let prefix: [String: Any] = ["system": messages[0], "tools": body["tools"] ?? []]
            return (try JSONSerialization.data(withJSONObject: prefix, options: [.sortedKeys]),
                    try XCTUnwrap(body["prompt_cache_key"] as? String))
        }
        let a = try await firstRequest()
        let b = try await firstRequest()
        XCTAssertEqual(a.prefix, b.prefix, "system + tools must be byte-identical across sessions")
        XCTAssertEqual(a.key, b.key, "prompt_cache_key is stable across sessions")
        XCTAssertEqual(a.prefix, FrogManifest.load().prefixBytes(supportsImages: false))
        XCTAssertTrue(a.key.hasPrefix(FrogManifest.load().cacheKey))
        // The system message is exactly the manifest's, nothing appended.
        let sys = try XCTUnwrap(try JSONSerialization.jsonObject(with: a.prefix) as? [String: Any])["system"] as? [String: Any]
        XCTAssertEqual(sys?["content"] as? String, FrogManifest.load().system)
    }

    func testRequestBodyIsKeySortedAndCarriesUserAgent() throws {
        let req = try FrogClient(config: config()).request(
            messages: [["role": "system", "content": "s"]], tools: FrogManifest.load().tools, cacheKey: "k")
        let body = try XCTUnwrap(req.httpBody)
        let resorted = try JSONSerialization.data(withJSONObject: JSONSerialization.jsonObject(with: body), options: [.sortedKeys])
        XCTAssertEqual(body, resorted)
        XCTAssertTrue(req.value(forHTTPHeaderField: "user-agent")?.hasPrefix("vcad/") == true)
    }

    func testZRangeParsesDescribeScene() {
        let line = "1 part(s), units mm, Z up:\n- 1:1 \"cube\" [cube] material=default bbox min (0.00, 0.00, 10.00) max (20.00, 20.00, 30.00)\n"
        let r = FrogLiveE2ETests.zRange(line)
        XCTAssertEqual(r?.0, 10); XCTAssertEqual(r?.1, 30)
    }

    // MARK: - config + manifest

    func testManifestMatchesEngineTools() {
        let m = FrogManifest.load()
        XCTAssertFalse(m.system.isEmpty)
        let names = m.tools.compactMap { ($0["function"] as? [String: Any])?["name"] as? String }
        for required in ["describe_scene", "inspect", "create", "translate", "place", "delete"] {
            XCTAssertTrue(names.contains(required), required)
        }
        XCTAssertEqual(m.offered(supportsImages: false).count, m.tools.count)
        XCTAssertEqual(m.offered(supportsImages: true).count, m.tools.count + m.visionTools.count)
    }

    func testRequestRefusesWithoutModelOrEndpoint() {
        var c = config(); c.model = ""
        XCTAssertThrowsError(try FrogClient(config: c).request(messages: [], tools: [], cacheKey: "k"))
        var d = config(); d.baseURL = ""
        XCTAssertThrowsError(try FrogClient(config: d).request(messages: [], tools: [], cacheKey: "k"))
        let ok = try? FrogClient(config: config()).request(messages: [], tools: [], cacheKey: "k")
        XCTAssertEqual(ok?.url?.absoluteString, "http://127.0.0.1:9/v1/chat/completions")
        XCTAssertEqual(ok?.value(forHTTPHeaderField: "authorization"), "Bearer test")
    }

    func testHTTPErrorsMapToContractCases() {
        let body = Data(#"{"error":{"message":"out of credit","type":"insufficient_quota","code":402}}"#.utf8)
        XCTAssertEqual(FrogError.from(status: 402, body: body, retryAfter: nil), .outOfCredit)
        XCTAssertEqual(FrogError.from(status: 401, body: Data(), retryAfter: nil), .unauthorized)
        XCTAssertEqual(FrogError.from(status: 503, body: Data(), retryAfter: "30"), .unavailable(retryAfter: 30))
        XCTAssertEqual(FrogError.from(status: 502, body: Data(#"{"error":{"message":"boom"}}"#.utf8), retryAfter: nil), .http(502, "boom"))
    }

    func testEnvOverridesSettings() {
        let defaults = UserDefaults(suiteName: "frog-test-\(UUID())")!
        defaults.set("http://settings/v1", forKey: FrogConfig.baseKey)
        defaults.set("m-settings", forKey: FrogConfig.modelKey)
        let none = FrogKeychain(service: "io.vcad.agent.test-\(UUID())", account: "none")
        var warnings: [String] = []
        let c = FrogConfig.load(defaults: defaults, env: ["FROG_BASE_URL": "http://env/v1", "FROG_API_KEY": "frog_abc"],
                                keychain: none, warn: { warnings.append($0) })
        XCTAssertEqual(c.baseURL, "http://env/v1")
        XCTAssertEqual(c.model, "m-settings")
        XCTAssertEqual(c.apiKey, "frog_abc")
        XCTAssertFalse(c.supportsImages)
        XCTAssertEqual(warnings, [])
    }

    func testDefaultsToFrogAPIWithNoModel() {
        let defaults = UserDefaults(suiteName: "frog-test-\(UUID())")!
        let none = FrogKeychain(service: "io.vcad.agent.test-\(UUID())", account: "none")
        let c = FrogConfig.load(defaults: defaults, env: [:], keychain: none, warn: { _ in })
        XCTAssertEqual(c.baseURL, "https://api.frog.toys/v1")
        XCTAssertEqual(c.model, "", "the model id is never hardcoded")
        XCTAssertNil(c.apiKey)
    }

    func testDeprecatedZestEnvStillWorksAndWarns() {
        let defaults = UserDefaults(suiteName: "frog-test-\(UUID())")!
        let none = FrogKeychain(service: "io.vcad.agent.test-\(UUID())", account: "none")
        var warnings: [String] = []
        let c = FrogConfig.load(defaults: defaults,
                                env: ["VCAD_ZEST_BASE": "http://old/v1", "VCAD_ZEST_KEY": "frog_old", "VCAD_ZEST_IMAGES": "1"],
                                keychain: none, warn: { warnings.append($0) })
        XCTAssertEqual(c.baseURL, "http://old/v1")
        XCTAssertEqual(c.apiKey, "frog_old")
        XCTAssertTrue(c.supportsImages)
        XCTAssertEqual(warnings.count, 3)
        XCTAssertTrue(warnings.allSatisfy { $0.contains("deprecated") }, "\(warnings)")
        // The new name wins over the alias, silently.
        warnings = []
        let d = FrogConfig.load(defaults: defaults, env: ["FROG_BASE_URL": "http://new/v1", "VCAD_ZEST_BASE": "http://old/v1"],
                                keychain: none, warn: { warnings.append($0) })
        XCTAssertEqual(d.baseURL, "http://new/v1")
        XCTAssertEqual(warnings, [])
    }

    func testKeyPrefixIsAWarningNotARefusal() {
        XCTAssertNil(FrogConfig.keyWarning("frog_abc"))
        XCTAssertNil(FrogConfig.keyWarning(nil))
        XCTAssertNotNil(FrogConfig.keyWarning("sk-other"))
        let defaults = UserDefaults(suiteName: "frog-test-\(UUID())")!
        let none = FrogKeychain(service: "io.vcad.agent.test-\(UUID())", account: "none")
        var warnings: [String] = []
        let c = FrogConfig.load(defaults: defaults, env: ["FROG_API_KEY": "sk-other"], keychain: none, warn: { warnings.append($0) })
        XCTAssertEqual(c.apiKey, "sk-other", "a foreign key is still used")
        XCTAssertEqual(warnings.count, 1)
        let req = try? FrogClient(config: FrogConfig(baseURL: "http://x/v1", model: "m", apiKey: "sk-other", supportsImages: false))
            .request(messages: [], tools: [], cacheKey: "k")
        XCTAssertEqual(req?.value(forHTTPHeaderField: "authorization"), "Bearer sk-other")
    }

    // MARK: - model discovery

    func testModelComesFromModelsListWhenUnset() async throws {
        let models = Data(#"{"object":"list","data":[{"id":"listed-model","object":"model"}]}"#.utf8)
        let transport = MockFrogTransport([{ _ in SSE.text(["hi"]) }], models: (models, 200))
        var c = config(); c.model = ""
        let a = FrogAgent(client: FrogClient(config: c, transport: transport), manifest: .load(),
                          executor: FrogToolExecutor(engine: DocumentEngine(replicaId: 1)))
        try await a.send("hello") { _ in }
        XCTAssertEqual(transport.plainRequests.first?.url?.absoluteString, "http://127.0.0.1:9/v1/models")
        XCTAssertEqual(transport.bodies.first?["model"] as? String, "listed-model")
    }

    func testConfiguredModelSkipsDiscovery() async throws {
        let transport = MockFrogTransport([{ _ in SSE.text(["hi"]) }])
        try await agent(transport, engine: DocumentEngine(replicaId: 1)).send("hello") { _ in }
        XCTAssertTrue(transport.plainRequests.isEmpty)
        XCTAssertEqual(transport.bodies.first?["model"] as? String, "test-model")
    }

    func testEmptyModelsListIsNotConfigured() async {
        let transport = MockFrogTransport([], models: (Data(#"{"data":[]}"#.utf8), 200))
        var c = config(); c.model = ""
        do {
            _ = try await FrogClient(config: c, transport: transport).resolveModel()
            XCTFail("expected notConfigured")
        } catch let e as FrogError {
            guard case .notConfigured = e else { return XCTFail("\(e)") }
        } catch { XCTFail("\(error)") }
    }

    // MARK: - render_view honours the requested view

    func testRenderViewParsesPresetsAndAngles() {
        XCTAssertEqual(FrogView([:]), .current)
        XCTAssertEqual(FrogView(["view": "current"]), .current)
        guard case let .angles(a, e) = FrogView(["view": "right"]) else { return XCTFail() }
        XCTAssertEqual(a, 90, accuracy: 1e-9); XCTAssertEqual(e, 0, accuracy: 1e-9)
        guard case let .angles(_, te) = FrogView(["view": "top"]) else { return XCTFail() }
        XCTAssertGreaterThan(te, 80)
        XCTAssertEqual(FrogView(["view": "top", "azimuth_deg": 10, "elevation_deg": 20]), .angles(azimuthDeg: 10, elevationDeg: 20))
        guard case let .angles(_, clamped) = FrogView(["elevation_deg": 120]) else { return XCTFail() }
        XCTAssertEqual(clamped, 85)
    }

    func testRenderViewPassesTheRequestedViewToTheRenderer() async {
        var seen: [FrogView] = []
        let ex = FrogToolExecutor(engine: DocumentEngine(replicaId: 1), supportsImages: true,
                                  snapshot: { view in seen.append(view); return Data([1]) })
        _ = await ex.execute(FrogToolCall(id: "1", name: "render_view", arguments: #"{"view":"front"}"#))
        let o = await ex.execute(FrogToolCall(id: "2", name: "render_view", arguments: #"{"azimuth_deg":45,"elevation_deg":30}"#))
        XCTAssertEqual(seen, [.angles(azimuthDeg: 0, elevationDeg: 0), .angles(azimuthDeg: 45, elevationDeg: 30)])
        XCTAssertTrue(o.content.contains("azimuth 45"), o.content)
        XCTAssertNotNil(o.image)
    }
}


/// A blank Untitled canvas gets an engine document on first agent use.
@MainActor
final class AgentBlankCanvasTests: XCTestCase {
    func testAgentEditsFromAnEmptyCanvas() async throws {
        let m = EditorModel()
        XCTAssertTrue(m.source.isSandbox)
        XCTAssertNil(m.engine)
        XCTAssertTrue(m.agentCanEdit)
        let engine = try XCTUnwrap(m.agentEnsureEngine())
        XCTAssertTrue(m.agentEnsureEngine() === engine, "idempotent")
        XCTAssertTrue(m.canEditDocument)
        let roots = { (m.documentJSON?["roots"] as? [Any])?.count ?? -1 }
        XCTAssertEqual(roots(), 0)

        let transport = MockFrogTransport([
            { _ in SSE.toolCall(id: "a", name: "create", args: SSE.json(["type": "cube", "params": ["size": ["x": 5, "y": 5, "z": 5]]])) },
            { _ in SSE.text(["made a cube"]) },
        ])
        let agent = FrogAgent(client: FrogClient(config: FrogConfig(baseURL: "http://127.0.0.1:9/v1", model: "m", apiKey: nil, supportsImages: false), transport: transport),
                              manifest: .load(), executor: FrogToolExecutor(engine: engine))
        let before = engine.save()
        let mutated = try await agent.send("a cube") { event in
            if case .toolFinished(_, let o) = event, let r = o.mutation { m.agentApplied(r) }
        }
        XCTAssertTrue(mutated)
        m.agentEndTurn(before: before)
        XCTAssertEqual(roots(), 1)
        XCTAssertTrue(m.usesDocumentTree)
        m.undo()
        XCTAssertEqual(roots(), 0)
    }
}

/// A whole agent turn is one undo step in the editor, however many tools ran.
@MainActor
final class AgentUndoTests: XCTestCase {
    func testAgentTurnIsOneUndoStep() async throws {
        let doc: [String: Any] = [
            "version": "0.1",
            "nodes": ["1": ["id": 1, "name": "Block", "op": ["type": "Cube", "size": ["x": 20, "y": 10, "z": 5]]]],
            "materials": [:], "part_materials": [:],
            "roots": [["root": 1, "material": "aluminum"]],
        ]
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString + ".vcad")
        try JSONSerialization.data(withJSONObject: doc).write(to: url)
        defer { try? FileManager.default.removeItem(at: url) }
        let m = EditorModel()
        m.openDocument(url)
        let engine = try XCTUnwrap(m.engine)
        let roots = { (m.documentJSON?["roots"] as? [Any])?.count ?? -1 }
        XCTAssertEqual(roots(), 1)

        let transport = MockFrogTransport([
            { _ in SSE.toolCall(id: "a", name: "create", args: SSE.json(["type": "sphere", "params": ["radius": 3]])) },
            { body in SSE.toolCall(id: "b", name: "translate",
                                   args: SSE.json(["child": lastCreatedId(body), "offset": ["x": 30, "y": 0, "z": 0]])) },
            { _ in SSE.toolCall(id: "c", name: "create", args: SSE.json(["type": "cylinder", "params": ["radius": 2, "height": 4]])) },
            { _ in SSE.text(["done"]) },
        ])
        let agent = FrogAgent(client: FrogClient(config: FrogConfig(baseURL: "http://127.0.0.1:9/v1", model: "m", apiKey: nil, supportsImages: false), transport: transport),
                              manifest: .load(), executor: FrogToolExecutor(engine: engine))
        let before = engine.save()
        let undoDepth = m.undoStack.count
        let mutated = try await agent.send("add two parts") { event in
            if case .toolFinished(_, let o) = event, let r = o.mutation { m.agentApplied(r) }
        }
        XCTAssertTrue(mutated)
        m.agentEndTurn(before: before)
        XCTAssertEqual(roots(), 3, "edits are live in the editor document")
        XCTAssertEqual(m.undoStack.count, undoDepth + 1)
        XCTAssertEqual(m.undoActionName, "Agent Edit")
        m.undo()
        XCTAssertEqual(roots(), 1, "one undo reverts the whole turn")
        m.redo()
        XCTAssertEqual(roots(), 3)
    }
}

final class FrogSSEParserTests: XCTestCase {
    let p = FrogSSEParser()

    func testTextDeltaAndDone() {
        XCTAssertEqual(p.parse(line: #"data: {"choices":[{"index":0,"delta":{"content":"Hi"},"finish_reason":null}]}"#), [.text("Hi")])
        XCTAssertEqual(p.parse(line: "data: [DONE]"), [.done])
        XCTAssertEqual(p.parse(line: "data:[DONE]\r"), [.done])
    }

    func testIgnoresCommentsBlanksAndOtherFields() {
        XCTAssertEqual(p.parse(line: ""), [])
        XCTAssertEqual(p.parse(line: ": keep-alive"), [])
        XCTAssertEqual(p.parse(line: "event: ping"), [])
        XCTAssertEqual(p.parse(line: "data: not json"), [])
    }

    func testToolCallFragmentsAccumulate() {
        var acc = FrogTurnAccumulator()
        let lines = [
            #"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"create","arguments":""}}]}}]}"#,
            #"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"type\":"}}]}}]}"#,
            #"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"id":"call_2","function":{"name":"delete","arguments":"{}"}}]}}]}"#,
            #"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"cube\"}"}}]}}]}"#,
            #"data: {"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
            #"data: {"choices":[],"usage":{"prompt_tokens":12,"completion_tokens":3}}"#,
            "data: [DONE]",
        ]
        for l in lines { p.parse(line: l).forEach { acc.apply($0) } }
        XCTAssertEqual(acc.toolCalls, [FrogToolCall(id: "call_1", name: "create", arguments: #"{"type":"cube"}"#),
                                       FrogToolCall(id: "call_2", name: "delete", arguments: "{}")])
        XCTAssertEqual(acc.finishReason, "tool_calls")
        XCTAssertTrue(acc.done)
        XCTAssertEqual(acc.text, "")
    }

    func testUsageAndErrorFrames() {
        XCTAssertEqual(p.parse(line: #"data: {"choices":[],"usage":{"prompt_tokens":7,"completion_tokens":2}}"#), [.usage(prompt: 7, completion: 2)])
        XCTAssertEqual(p.parse(line: #"data: {"error":{"message":"overloaded","type":"api_error"}}"#), [.error("overloaded")])
    }

    func testMissingCallIdsGetStableFallbacks() {
        var acc = FrogTurnAccumulator()
        acc.apply(.toolCall(index: 0, id: nil, name: "describe_scene", arguments: ""))
        XCTAssertEqual(acc.toolCalls.first?.id, "call_0")
        let msg = acc.assistantMessage
        let call = (msg["tool_calls"] as? [[String: Any]])?.first
        XCTAssertEqual(call?["id"] as? String, "call_0")
        XCTAssertEqual((call?["function"] as? [String: Any])?["arguments"] as? String, "{}")
    }
}
