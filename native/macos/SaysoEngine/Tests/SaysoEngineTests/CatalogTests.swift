import XCTest

@testable import SaysoEngine

final class CatalogTests: XCTestCase {
    private let json = """
        {"models": [
          {"id": "old-mac", "name": "Old Mac", "engine": {"kind": "parakeet_tdt", "version": "v2"},
           "size_bytes": 10, "languages": ["en"], "roles": ["final"], "timestamps": "word",
           "recommended": true, "min_macos": 14},
          {"id": "new-mac", "name": "New Mac", "engine": {"kind": "apple_speech"},
           "size_bytes": 0, "languages": ["en"], "roles": ["final"], "timestamps": "none",
           "recommended": false, "min_macos": 26}],
         "retired": [{"id": "gone", "message": "Use old-mac."}]}
        """

    private func errorCode(_ body: () throws -> Any) -> (String?, String?) {
        do {
            _ = try body()
            return (nil, nil)
        } catch let error as EngineError {
            return (error.code, error.message)
        } catch {
            return (nil, "\(error)")
        }
    }

    func testParsesModelsAndRetired() throws {
        let catalog = try Catalog(json: json, macOS: 26)
        XCTAssertEqual(catalog.models.map(\.id), ["old-mac", "new-mac"])
        XCTAssertEqual(catalog.models[0].spec, .parakeetTdt(.v2))
        XCTAssertEqual(catalog.retired.map(\.id), ["gone"])
        XCTAssertEqual(catalog.retired[0].message, "Use old-mac.")
    }

    func testLeavesOutModelsThatNeedANewerMacOS() throws {
        XCTAssertEqual(try Catalog(json: json, macOS: 14).models.map(\.id), ["old-mac"])
        XCTAssertEqual(try Catalog(json: json, macOS: 25).models.map(\.id), ["old-mac"])
        XCTAssertEqual(try Catalog(json: json, macOS: 26).models.map(\.id), ["old-mac", "new-mac"])
    }

    func testReplyHasTheSpecFieldsAndNoMinMacOS() throws {
        let reply = try Catalog(json: json, macOS: 26).reply
        let models = try XCTUnwrap(reply["models"] as? [[String: Any]])
        XCTAssertEqual(models.count, 2)
        for entry in models {
            XCTAssertEqual(
                Set(entry.keys),
                ["id", "name", "engine", "size_bytes", "languages", "roles", "timestamps", "recommended"])
        }
        let retired = try XCTUnwrap(reply["retired"] as? [[String: String]])
        XCTAssertEqual(retired, [["id": "gone", "message": "Use old-mac."]])
    }

    func testLookupByIdAndItsTwoErrors() throws {
        let catalog = try Catalog(json: json, macOS: 14)
        XCTAssertEqual(try catalog.spec(of: "old-mac"), .parakeetTdt(.v2))

        // A model for a newer macOS is not in the catalog of this Mac.
        let unknown = errorCode { try catalog.spec(of: "new-mac") }
        XCTAssertEqual(unknown.0, "unknown_model")
        XCTAssertEqual(errorCode { try catalog.spec(of: "no-such-model") }.0, "unknown_model")

        let retired = errorCode { try catalog.spec(of: "gone") }
        XCTAssertEqual(retired.0, "model_retired")
        XCTAssertTrue(try XCTUnwrap(retired.1).contains("Use old-mac."))
    }

    func testRejectsABrokenCatalog() {
        XCTAssertThrowsError(try Catalog(json: "[]", macOS: 14))
        XCTAssertThrowsError(try Catalog(json: #"{"models": [{"id": "m"}], "retired": []}"#, macOS: 14))
        XCTAssertThrowsError(
            try Catalog(
                json: #"{"models": [{"id": "m", "engine": {"kind": "fly"}, "min_macos": 14}], "retired": []}"#,
                macOS: 14))
        XCTAssertThrowsError(try Catalog(json: #"{"models": [], "retired": [{"id": "m"}]}"#, macOS: 14))
    }

    func testRequestUsesItsEngineOrTheCatalog() throws {
        let catalog = try Catalog(json: json, macOS: 14)
        let own = try Request(line: #"{"type":"load","model":"old-mac","engine":{"kind":"parakeet_tdt","version":"v3"}}"#)
        XCTAssertEqual(try own.engine(of: "old-mac", catalog: catalog), .parakeetTdt(.v3))
        // An engine object also works for an id that the catalog does not have.
        XCTAssertEqual(try own.engine(of: "other", catalog: catalog), .parakeetTdt(.v3))
        for line in [#"{"type":"load","model":"old-mac"}"#, #"{"type":"load","model":"old-mac","engine":null}"#] {
            XCTAssertEqual(try Request(line: line).engine(of: "old-mac", catalog: catalog), .parakeetTdt(.v2))
        }
        let missing = try Request(line: #"{"type":"load","model":"x"}"#)
        XCTAssertEqual(errorCode { try missing.engine(of: "x", catalog: catalog) }.0, "unknown_model")
    }

    func testListModelsEntriesMayLeaveOutEngine() throws {
        let catalog = try Catalog(json: json, macOS: 14)
        let request = try Request(
            line: #"{"type":"list_models","models":[{"id":"old-mac"},{"id":"x","engine":{"kind":"cohere"}}]}"#)
        let models = try XCTUnwrap(try request.models(catalog: catalog))
        XCTAssertEqual(models.map(\.id), ["old-mac", "x"])
        XCTAssertEqual(models.map(\.spec), [.parakeetTdt(.v2), .cohere])
        XCTAssertNil(try Request(line: #"{"type":"list_models"}"#).models(catalog: catalog))
        XCTAssertNil(try Request(line: #"{"type":"list_models","models":null}"#).models(catalog: catalog))
        let bad = try Request(line: #"{"type":"list_models","models":[{"id":"no-such-model"}]}"#)
        XCTAssertEqual(errorCode { try bad.models(catalog: catalog) as Any }.0, "unknown_model")
    }

    /// The generated catalog parses, and the Swift engine runs each of its models.
    func testBuiltInCatalogParses() throws {
        let all = try Catalog(json: BuiltInCatalog.json, macOS: 99)
        XCTAssertFalse(all.models.isEmpty)
        XCTAssertEqual(Set(all.models.map(\.id)).count, all.models.count, "ids are unique")
        let here = try Catalog.builtIn()
        XCTAssertFalse(here.models.isEmpty)
        XCTAssertLessThanOrEqual(here.models.count, all.models.count)
    }
}
