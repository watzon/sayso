// The result of diarization (docs/engine-protocol.md, "Diarization").
//
// The offline diarizer of FluidAudio gives turns with its own speaker labels, in seconds.
// `Diarization.finish` makes the turns of the protocol: whole milliseconds, speakers
// numbered from 0 in the order in which they first speak, the turns of one speaker joined
// where they touch or overlap, and the turns in order.
import CoreML
import FluidAudio
import Foundation

enum Diarization {
    /// The models of an artifact in the model store. `OfflineDiarizerModels.load` can delete
    /// the folder and download it again (docs/sdk.md, finding 3), so this reads the folder
    /// only, with the compute units of that function.
    static func loadStored(from folder: URL) throws -> OfflineDiarizerModels {
        let start = Date()
        func model(_ name: String, units: MLComputeUnits) throws -> MLModel {
            let configuration = MLModelConfiguration()
            configuration.computeUnits = units
            return try MLModel(contentsOf: folder.appendingPathComponent(name), configuration: configuration)
        }
        let names = ModelNames.OfflineDiarizer.self
        let segmentation = try model(names.segmentationPath, units: .all)
        let embedding = try model(names.embeddingPath, units: .all)
        let plda = try model(names.pldaRhoPath, units: .all)
        let fbank = try model(names.fbankPath, units: .cpuOnly)
        return OfflineDiarizerModels(
            segmentationModel: segmentation, fbankModel: fbank, embeddingModel: embedding, pldaRhoModel: plda,
            pldaPsi: try pldaPsi(in: folder), compilationDuration: Date().timeIntervalSince(start))
    }

    /// The tensor `psi` of the PLDA parameters: base64 of 32-bit floats.
    private static func pldaPsi(in folder: URL) throws -> [Double] {
        let data = try Data(contentsOf: folder.appendingPathComponent(ModelNames.OfflineDiarizer.pldaParameters))
        guard let root = try JSONSerialization.jsonObject(with: data) as? [String: Any],
            let tensors = root["tensors"] as? [String: Any], let psi = tensors["psi"] as? [String: Any],
            let base64 = psi["data_base64"] as? String,
            let decoded = Data(base64Encoded: base64, options: [.ignoreUnknownCharacters]),
            decoded.count >= MemoryLayout<Float>.size
        else { throw EngineError("the PLDA parameters have no psi tensor") }
        var floats = [Float](repeating: 0, count: decoded.count / MemoryLayout<Float>.size)
        _ = floats.withUnsafeMutableBytes { decoded.copyBytes(to: $0) }
        return floats.map(Double.init)
    }

    /// A turn as the model gives it. The speaker is the label of the model.
    struct RawTurn: Equatable {
        let start: Double
        let end: Double
        let speaker: String
    }

    /// A turn of the protocol.
    struct Turn: Equatable {
        let startMs: Int
        let endMs: Int
        let speaker: Int
    }

    /// The reply of `diarize`.
    struct Result: Equatable {
        let speakers: Int
        let turns: [Turn]

        var json: [String: Any] {
            [
                "speakers": speakers,
                "turns": turns.map { ["start_ms": $0.startMs, "end_ms": $0.endMs, "speaker": $0.speaker] },
            ]
        }
    }

    /// Make the turns of the protocol. `audioMs` is the length of the audio: no turn passes it.
    static func finish(_ raw: [RawTurn], audioMs: Int) -> Result {
        func ms(_ seconds: Double) -> Int {
            guard seconds.isFinite else { return 0 }
            return min(max(Int((seconds * 1000).rounded()), 0), audioMs)
        }
        let turns = raw
            .map { (start: ms($0.start), end: ms($0.end), speaker: $0.speaker) }
            .filter { $0.end > $0.start }
            // The order of the first turns gives the numbers.
            .sorted { ($0.start, $0.end, $0.speaker) < ($1.start, $1.end, $1.speaker) }
        var numbers: [String: Int] = [:]
        for turn in turns where numbers[turn.speaker] == nil {
            numbers[turn.speaker] = numbers.count
        }
        // Join the turns of each speaker. The turns of one speaker are in start order now.
        var joined: [Turn] = []
        var lastOf: [Int: Int] = [:]
        for turn in turns {
            let speaker = numbers[turn.speaker]!
            if let at = lastOf[speaker], turn.start <= joined[at].endMs {
                let last = joined[at]
                joined[at] = Turn(startMs: last.startMs, endMs: max(last.endMs, turn.end), speaker: speaker)
            } else {
                lastOf[speaker] = joined.count
                joined.append(Turn(startMs: turn.start, endMs: turn.end, speaker: speaker))
            }
        }
        joined.sort { ($0.startMs, $0.endMs, $0.speaker) < ($1.startMs, $1.endMs, $1.speaker) }
        return Result(speakers: numbers.count, turns: joined)
    }
}
