import XCTest

@testable import SaysoEngine

/// The migration in Sayso (`crates/sayso-core/src/model_migration.rs`) finds the files of a
/// model in the folders that this engine used before the model store. Its list must say the
/// same as `ModelStore`.
final class OldFoldersTests: XCTestCase {
    func testTheMigrationLooksInTheFoldersOfThisEngine() throws {
        let root = URL(fileURLWithPath: "/old", isDirectory: true)
        let store = ModelStore(root: root)
        func relative(_ url: URL) -> String { String(url.path.dropFirst(root.path.count + 1)) }
        let catalog = try Catalog.builtIn()
        XCTAssertFalse(migrationOldFolders.isEmpty)
        for entry in migrationOldFolders {
            guard let model = catalog.models.first(where: { $0.id == entry.id }) else { continue }
            var want: [String: String] = [:]
            switch model.spec {
            case .parakeetUnified:
                want = ["model": relative(store.unifiedDir), "ctc": relative(store.ctcDir)]
            case .parakeetEou:
                want = ["model": relative(store.eouDir)]
            case .whisper(let variant):
                let tokenizer = try XCTUnwrap(ModelStore.whisperTokenizer(variant: variant))
                want = ["model": relative(store.whisperDir(variant)), "tokenizer": "models/\(tokenizer.repo)"]
            case .cohere:
                let folder = relative(try XCTUnwrap(store.ownFolder(model.spec)))
                want = ["model": folder, "vocabulary": folder]
            default:
                want = ["model": relative(try XCTUnwrap(store.ownFolder(model.spec)))]
            }
            let have = Dictionary(uniqueKeysWithValues: entry.parts.map { ($0.role, $0.folder) })
            XCTAssertEqual(have, want, entry.id)
        }
    }

    func testEachModelWithFilesHasADescriptorAndAnOldFolder() throws {
        let data = try StoreDescriptors.load(path: nil)
        for model in try Catalog.builtIn().models {
            guard let format = model.spec.storeFormat else { continue }
            XCTAssertNotNil(data.find(model: model.id, format: format), model.id)
            XCTAssertTrue(migrationOldFolders.contains { $0.id == model.id }, model.id)
        }
    }
}
