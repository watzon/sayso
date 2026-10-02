#!/usr/bin/env python3
"""Throwaway NDJSON driver for SaysoEngine. Usage: drive.py <label> [engine ...]
Runs one engine process through load + transcribe/stream scenarios, writes runs/<label>.jsonl."""
import json, os, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
BIN = os.environ.get("ENGINE_BIN", f"{HERE}/Engine/.build/release/SaysoEngine")
MODELS = f"{HERE}/.models"
SHORT, LONG = f"{HERE}/audio/short.wav", f"{HERE}/audio/long30.wav"
label = sys.argv[1]
engines = sys.argv[2:] or ["parakeet_unified_batch", "parakeet_unified_stream", "whisper"]

def du(path):
    try: return int(subprocess.check_output(["du", "-sk", path]).split()[0]) * 1024
    except Exception: return 0

os.makedirs(f"{HERE}/runs", exist_ok=True)
log = open(f"{HERE}/runs/{label}.jsonl", "w")
t_launch = time.time()
p = subprocess.Popen([BIN], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=open(f"{HERE}/runs/{label}.stderr", "w"),
                     env={**os.environ, "SAYSO_MODELS_DIR": MODELS}, text=True, bufsize=1, cwd=HERE)
rid = 0

def rss_mb():
    out = subprocess.run(["ps", "-o", "rss=", "-p", str(p.pid)], capture_output=True, text=True).stdout.strip()
    return round(int(out) / 1024) if out else None

def call(req, quiet_progress=True):
    """Send request, return all events until final/error/ready/log-bye. Adds t (wall seconds) to each."""
    global rid
    rid += 1
    req = {"v": 1, "id": f"r{rid}", **req}
    t0 = time.time()
    p.stdin.write(json.dumps(req) + "\n"); p.stdin.flush()
    events = []
    while True:
        line = p.stdout.readline()
        if not line: raise RuntimeError("engine closed stdout")
        ev = json.loads(line); ev["t"] = round(time.time() - t0, 3)
        log.write(json.dumps(ev) + "\n")
        events.append(ev)
        if ev["type"] in ("final", "error"): return events
        if ev["type"] == "model_state" and ev.get("state") == "ready": return events

# wait for the startup log line
first = json.loads(p.stdout.readline()); first["t"] = round(time.time() - t_launch, 3)
log.write(json.dumps(first) + "\n")
print(f"[{label}] engine up, first line at {first['t']}s")

for eng in engines:
    before = du(MODELS)
    req = {"type": "load_model", "engine": eng}
    t0 = time.time()
    ev = call(req)
    wall = time.time() - t0
    last = ev[-1]
    if last["type"] == "error": print("LOAD ERROR", last); continue
    prog = [e for e in ev if e["type"] == "download_progress"]
    print(f"[{label}] load {eng}: wall={wall:.2f}s download_ms={last.get('download_ms'):.0f} load_ms={last.get('load_ms'):.0f} "
          f"progress_events={len(prog)} disk_delta={(du(MODELS)-before)/1e6:.1f}MB rss={rss_mb()}MB")
    for clip, path in (("short", SHORT), ("long", LONG)):
        if eng == "parakeet_unified_stream":
            for rt in (False, True):
                ev = call({"type": "stream_file", "path": path, "chunk_ms": 160, "realtime": rt, "engine": eng})
                fin = ev[-1]
                parts = [e for e in ev if e["type"] == "partial"]
                if fin["type"] == "error": print("ERR", fin); continue
                first_lat = parts[0]["since_chunk_ms"] if parts else None
                first_audio = parts[0]["audio_ms"] if parts else None
                gaps = [b["audio_ms"] - a["audio_ms"] for a, b in zip(parts, parts[1:])]
                print(f"[{label}]  stream {clip} realtime={rt}: total_ms={fin['total_ms']:.0f} finish_ms={fin['ms']:.0f} "
                      f"audio_ms={fin['audio_ms']:.0f} partials={len(parts)} first_partial_at_audio_ms={first_audio} "
                      f"first_partial_since_chunk_ms={first_lat:.0f} median_gap_audio_ms={sorted(gaps)[len(gaps)//2] if gaps else None}")
                print(f"[{label}]   text: {fin['text']}")
        else:
            for n in (1, 2):
                ev = call({"type": "transcribe_file", "path": path, "engine": eng})
                fin = ev[-1]
                if fin["type"] == "error": print("ERR", fin); continue
                print(f"[{label}]  transcribe {clip} run{n}: {fin['ms']:.0f} ms")
                if n == 1: print(f"[{label}]   text: {fin['text']}")
p.stdin.write(json.dumps({"v": 1, "id": "bye", "type": "shutdown"}) + "\n"); p.stdin.flush()
p.wait(timeout=30)
print(f"[{label}] done, models dir = {du(MODELS)/1e6:.1f} MB")
