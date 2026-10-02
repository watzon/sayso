// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "SaysoEngine",
    platforms: [.macOS(.v14)],
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
            swiftSettings: [.swiftLanguageMode(.v5)]
        )
    ]
)
