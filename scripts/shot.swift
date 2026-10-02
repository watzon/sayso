// Screenshot the largest on-screen window of a process: swift scripts/shot.swift <owner-name> <out.png> [min-width]
import CoreGraphics
import Foundation
let args = CommandLine.arguments
let owner = args.count > 1 ? args[1] : "sayso"
let out = args.count > 2 ? args[2] : "/tmp/sayso-shot.png"
let minW = args.count > 3 ? Double(args[3]) ?? 200 : 200
let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
let wins = list.filter { ($0[kCGWindowOwnerName as String] as? String) == owner }
  .compactMap { w -> (Int, Double)? in
    guard let id = w[kCGWindowNumber as String] as? Int, let b = w[kCGWindowBounds as String] as? [String: Double] else { return nil }
    let width = b["Width"] ?? 0
    return width >= minW ? (id, width * (b["Height"] ?? 0)) : nil
  }.sorted { $0.1 > $1.1 }
guard let id = wins.first?.0 else { print("no window for \(owner)"); exit(1) }
let p = Process(); p.launchPath = "/usr/sbin/screencapture"; p.arguments = ["-x", "-o", "-l", "\(id)", out]; p.launch(); p.waitUntilExit()
print(out)
