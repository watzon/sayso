// SaysoSpike: tiny LSUIElement app. Logs mic authorization, asks for it if undetermined,
// spawns the bundled sidecar, asks it for its own view of the mic status, then quits.
import AVFoundation
import AppKit

let logPath = "/tmp/sayso-spike.log"
func log(_ s: String) {
    let line = "\(ISO8601DateFormatter().string(from: Date())) [app pid \(getpid())] \(s)\n"
    FileHandle.standardError.write(Data(line.utf8))
    if let h = FileHandle(forWritingAtPath: logPath) { h.seekToEndOfFile(); h.write(Data(line.utf8)); try? h.close() }
    else { try? line.write(toFile: logPath, atomically: true, encoding: .utf8) }
}
func statusName(_ s: AVAuthorizationStatus) -> String {
    ["notDetermined", "restricted", "denied", "authorized"][s.rawValue]
}

func runSidecar() {
    guard let dir = Bundle.main.executableURL?.deletingLastPathComponent() else { return }
    let p = Process()
    p.executableURL = dir.appendingPathComponent("SaysoEngine")
    let inPipe = Pipe(), outPipe = Pipe()
    p.standardInput = inPipe; p.standardOutput = outPipe
    do { try p.run() } catch { log("sidecar spawn failed: \(error)"); return }
    log("spawned sidecar pid \(p.processIdentifier)")
    inPipe.fileHandleForWriting.write(Data("{\"v\":1,\"id\":\"m1\",\"type\":\"mic_status\"}\n{\"v\":1,\"id\":\"bye\",\"type\":\"shutdown\"}\n".utf8))
    p.waitUntilExit()
    let out = String(data: outPipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
    for l in out.split(separator: "\n") { log("sidecar says: \(l)") }
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
DispatchQueue.main.async {
    let before = AVCaptureDevice.authorizationStatus(for: .audio)
    log("AVCaptureDevice.authorizationStatus(.audio) at launch = \(statusName(before))")
    func finish() {
        runSidecar()
        log("done")
        exit(0)
    }
    if before == .notDetermined {
        log("requesting microphone access (a prompt should appear)")
        AVCaptureDevice.requestAccess(for: .audio) { granted in
            log("requestAccess result: granted=\(granted), status now \(statusName(AVCaptureDevice.authorizationStatus(for: .audio)))")
            finish()
        }
    } else {
        finish()
    }
}
app.run()
