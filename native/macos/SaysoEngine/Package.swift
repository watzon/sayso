// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "SaysoEngine",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "SaysoEngine", targets: ["SaysoEngine"])
    ],
    dependencies: [
        .package(url: "https://github.com/argmaxinc/argmax-oss-swift", exact: "1.1.0"),
        .package(url: "https://github.com/FluidInference/FluidAudio", exact: "0.17.5"),
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
        )
    ]
)
