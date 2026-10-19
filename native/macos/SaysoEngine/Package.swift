// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "SaysoEngine",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "SaysoEngine", targets: ["SaysoEngine"])
    ],
    dependencies: [
        .package(url: "https://github.com/argmaxinc/argmax-oss-swift", exact: "1.1.0"),
        // No traits: the default trait links the NeMo text normalizer, a Rust
        // staticlib of 8 MB that only the TTS frontends and ITN use. The engine
        // needs neither. Traits need tools 6.2.
        .package(url: "https://github.com/FluidInference/FluidAudio", exact: "0.17.5", traits: []),
    ],
    targets: [
        .executableTarget(
            name: "SaysoEngine",
            dependencies: [
                .product(name: "WhisperKit", package: "argmax-oss-swift"),
                .product(name: "FluidAudio", package: "FluidAudio"),
            ],
            // Engine state lives in actors, but FluidAudio and WhisperKit hand out
            // non-Sendable types (AVAudioPCMBuffer, WhisperKit), so Swift 6 strict
            // checking would force `@unchecked` wrappers around every call.
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .testTarget(
            name: "SaysoEngineTests",
            dependencies: ["SaysoEngine"],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
    ]
)
