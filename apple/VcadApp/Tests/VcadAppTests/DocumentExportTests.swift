import XCTest
@testable import VcadApp

/// File ▸ Export and Render Photoreal… through vcad-ffi.
final class DocumentExportTests: XCTestCase {
    private let doc = Data("""
    {"version":"0.1","materials":{},"part_materials":{},
     "nodes":{"1":{"id":1,"name":"Block","op":{"type":"Cube","size":{"x":10,"y":20,"z":5}}}},
     "roots":[{"root":1,"material":"default"}]}
    """.utf8)

    private func tempURL(_ ext: String) -> URL {
        FileManager.default.temporaryDirectory.appendingPathComponent("vcad-export-\(UUID().uuidString).\(ext)")
    }

    func testExportsEveryFormat() throws {
        for format in DocumentExportFormat.allCases {
            let url = tempURL(format.fileExtension)
            defer { try? FileManager.default.removeItem(at: url) }
            let result = KernelExport.export(json: doc, baseDir: nil, to: url, format: format)
            XCTAssertEqual(try result.get(), 1, "\(format.label)")
            let data = try Data(contentsOf: url)
            switch format {
            case .stl:
                let tris = data.subdata(in: 80..<84).withUnsafeBytes { $0.loadUnaligned(as: UInt32.self) }
                XCTAssertEqual(data.count, 84 + Int(tris) * 50)
            case .step:
                XCTAssertTrue(String(decoding: data, as: UTF8.self).hasPrefix("ISO-10303-21;"))
            case .glb:
                XCTAssertEqual(data.prefix(4), Data("glTF".utf8))
            }
        }
    }

    func testExportErrorIsReported() {
        let url = tempURL("stl")
        let result = KernelExport.export(json: Data("{".utf8), baseDir: nil, to: url, format: .stl)
        guard case let .failure(error) = result else { return XCTFail("expected failure") }
        XCTAssertTrue(error.message.contains("parse"), error.message)
        XCTAssertFalse(FileManager.default.fileExists(atPath: url.path))
    }

    func testPhotorealCPURendersPNGWithProgress() throws {
        final class Box: @unchecked Sendable { var calls = 0 }
        let box = Box()
        let job = PhotorealJob { _, _ in box.calls += 1 }
        let png = try job.render(json: doc, baseDir: nil, width: 32, height: 24, samples: 2,
                                 azimuth: 35, elevation: 20, forceCPU: true).get()
        XCTAssertEqual(png.prefix(4), Data([0x89, 0x50, 0x4E, 0x47]))
        XCTAssertGreaterThanOrEqual(box.calls, 2)   // start + finish
    }

    /// GPU tracer (CPU fallback on a machine without an adapter): batched
    /// progress, monotonically increasing, ending at the requested total.
    func testPhotorealDefaultPathReportsBatches() throws {
        final class Box: @unchecked Sendable { var seen: [UInt32] = [] }
        let box = Box()
        let job = PhotorealJob { done, total in
            XCTAssertEqual(total, 48)
            box.seen.append(done)
        }
        let png = try job.render(json: doc, baseDir: nil, width: 32, height: 24, samples: 48,
                                 azimuth: 35, elevation: 20).get()
        XCTAssertEqual(png.prefix(4), Data([0x89, 0x50, 0x4E, 0x47]))
        XCTAssertEqual(box.seen.first, 0)
        XCTAssertEqual(box.seen.last, 48)
        XCTAssertEqual(box.seen, box.seen.sorted())
    }

    func testPhotorealCancel() {
        let job = PhotorealJob()
        job.cancel()
        let result = job.render(json: doc, baseDir: nil, width: 32, height: 24, samples: 2,
                                azimuth: 35, elevation: 20, forceCPU: true)
        guard case let .failure(error) = result else { return XCTFail("expected cancel") }
        XCTAssertTrue(error.isCancelled, error.message)
    }
}
