// SenseVoice Small with the fp32 encoder.
//
// The int8 and fp16 encoders compute in fp16, and on some clear speech every logit came
// out NaN, so the text was empty (about 10% of the benchmark clips, on the Neural Engine
// and on the CPU). The fp32 encoder gives text for the same clips. It takes a fixed
// input of 1800 frames (30 s), and FluidAudio's `SenseVoiceManager` sends smaller
// buckets, so this file runs the models itself.
import Accelerate
import CoreML
import FluidAudio
import Foundation

enum SenseVoice {
    static let precision = SenseVoiceEncoderPrecision.fp32
    /// The input length of the fp32 encoder, in frames of 60 ms.
    static let frames = 1800
    /// The longest piece of audio that fits the encoder, with a margin.
    static let maxSeconds = 25
    /// "With ITN": the model writes punctuation, capitals, and digits.
    static let textNorm: Int32 = 14

    /// The encoder file of the int8 build that Sayso used before. A download removes it.
    static let oldEncoder = "SenseVoiceSmall_int8.mlmodelc"

    static func encoderExists(in folder: URL) -> Bool {
        FileManager.default.fileExists(atPath: folder.appendingPathComponent("SenseVoiceSmall_fp32.mlmodelc").path)
    }

    /// The text of one piece of audio (at most `maxSeconds`), without the model's tags, and
    /// the language code from the first tag of the model (`<|en|>`), when it has one.
    static func transcribe(
        models: SenseVoiceModels, audio: [Float], language: Int32
    ) throws -> (text: String, language: String?) {
        let features = try preprocess(models: models, audio: audio)
        let count = min(features.shape[1].intValue, frames)
        let logits = try encode(models: models, features: features, count: count, language: language)
        let ids = ctc(logits: logits, frames: count + SenseVoiceConfig.numQueryTokens)
        let tagged = decodeCtcTokenIds(ids, vocabulary: models.vocabulary)
        let text = tagged
            .replacingOccurrences(of: "<\\|[^|]*\\|>", with: "", options: .regularExpression)
            .trimmingCharacters(in: .whitespaces)
        return (text, languageTag(in: tagged))
    }

    /// The language codes of the model. The model writes one of them first (`<|zh|>`).
    private static let languageCodes: Set<String> = ["zh", "en", "yue", "ja", "ko"]

    /// The language code of the first tag of `tagged`, or nil. `<|nospeech|>` and other
    /// tags give nil.
    static func languageTag(in tagged: String) -> String? {
        guard let match = tagged.range(of: "<\\|[a-z]+\\|>", options: .regularExpression) else { return nil }
        let code = String(tagged[match].dropFirst(2).dropLast(2))
        return languageCodes.contains(code) ? code : nil
    }

    /// waveform [1, N] in the int16 range -> features [1, T, 560].
    private static func preprocess(models: SenseVoiceModels, audio: [Float]) throws -> MLMultiArray {
        let waveform = try MLMultiArray(shape: [1, audio.count as NSNumber], dataType: .float32)
        let pointer = waveform.dataPointer.assumingMemoryBound(to: Float32.self)
        var scale = SenseVoiceConfig.waveformScale
        vDSP_vsmul(audio, 1, &scale, pointer, 1, vDSP_Length(audio.count))
        let input = try MLDictionaryFeatureProvider(dictionary: ["waveform": MLFeatureValue(multiArray: waveform)])
        guard let features = try models.preprocessor.prediction(from: input).featureValue(for: "features")?.multiArrayValue
        else { throw EngineError("SenseVoice gave no features") }
        return features
    }

    /// features -> CTC logits [1, 4 + frames, vocabulary]. The rest of the 1800 frames are zero.
    private static func encode(models: SenseVoiceModels, features: MLMultiArray, count: Int, language: Int32) throws -> MLMultiArray {
        let dim = SenseVoiceConfig.featureDim
        let speech = try MLMultiArray(shape: [1, frames as NSNumber, dim as NSNumber], dataType: .float32)
        let target = speech.dataPointer.assumingMemoryBound(to: Float32.self)
        memset(target, 0, frames * dim * MemoryLayout<Float32>.size)
        if features.dataType == .float32 {
            memcpy(target, features.dataPointer, count * dim * MemoryLayout<Float32>.size)
        } else {
            for i in 0..<(count * dim) { target[i] = features[i].floatValue }
        }
        func scalar(_ value: Int32) throws -> MLMultiArray {
            let array = try MLMultiArray(shape: [1], dataType: .int32)
            array[0] = NSNumber(value: value)
            return array
        }
        let input = try MLDictionaryFeatureProvider(dictionary: [
            "speech": MLFeatureValue(multiArray: speech),
            "speech_lengths": MLFeatureValue(multiArray: try scalar(Int32(count))),
            "language": MLFeatureValue(multiArray: try scalar(language)),
            "textnorm": MLFeatureValue(multiArray: try scalar(textNorm)),
        ])
        guard let logits = try models.encoder.prediction(from: input).featureValue(for: "ctc_logits")?.multiArrayValue
        else { throw EngineError("SenseVoice gave no logits") }
        return logits
    }

    /// Greedy CTC: the best token of each frame, without blanks and repeats.
    static func ctc(logits: MLMultiArray, frames: Int) -> [Int] {
        let vocab = logits.shape[2].intValue
        let stride = logits.strides[1].intValue
        let count = min(frames, logits.shape[1].intValue)
        var row = [Float](repeating: 0, count: vocab)
        var ids: [Int] = []
        var previous = -1
        for t in 0..<count {
            switch logits.dataType {
            case .float32:
                let base = logits.dataPointer.assumingMemoryBound(to: Float32.self) + t * stride
                row.withUnsafeMutableBufferPointer { $0.baseAddress!.update(from: base, count: vocab) }
            default:
                for k in 0..<vocab { row[k] = logits[t * stride + k].floatValue }
            }
            var best: Float = 0
            var index: vDSP_Length = 0
            vDSP_maxvi(row, 1, &best, &index, vDSP_Length(vocab))
            let id = Int(index)
            if id != SenseVoiceConfig.blankId && id != previous { ids.append(id) }
            previous = id
        }
        return ids
    }

    /// The language id of SenseVoice. Zero lets the model detect the language.
    static func language(_ code: String) -> Int32 {
        switch code {
        case "zh": return 3
        case "en": return 4
        case "yue": return 7
        case "ja": return 11
        case "ko": return 12
        default: return SenseVoiceConfig.defaultLanguage
        }
    }
}
