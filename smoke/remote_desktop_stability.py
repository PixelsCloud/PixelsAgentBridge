"""Summarize real native desktop probe JSONL without storing desktop images."""
import argparse
import json
from pathlib import Path
import statistics


def percentile(values, fraction):
    if not values:
        return None
    values = sorted(values)
    index = (len(values) - 1) * fraction
    low = int(index)
    high = min(low + 1, len(values) - 1)
    return values[low] + (values[high] - values[low]) * (index - low)


def summarize(path, warmup_seconds=5):
    raw = path.read_bytes()
    # Windows PowerShell 5 redirection writes UTF-16; direct process capture
    # and PowerShell 7 write UTF-8. Both are normal supported Windows paths.
    text = raw.decode("utf-16" if raw.startswith((b"\xff\xfe", b"\xfe\xff")) else "utf-8-sig")
    events = [json.loads(line) for line in text.splitlines() if line.strip()]
    opened = next((event for event in events if event.get("event") == "opened"), None)
    closed = next((event for event in events if event.get("event") == "closed"), None)
    telemetry = events[-1] if events and events[-1].get("event") == "telemetry" else None
    errors = [event for event in events if event.get("event") == "error"]
    measurement_complete = bool(telemetry and telemetry.get("dropped_events") == 0
                                and telemetry.get("written_events") == len(events) - 1)
    presented = [event for event in events if event.get("event") == "presented"
                 and event["elapsed_us"] >= warmup_seconds * 1_000_000]
    diagnostics = [event for event in events if event.get("event") == "diagnostics"
                   and event["elapsed_us"] >= warmup_seconds * 1_000_000]
    gaps = [(right["elapsed_us"] - left["elapsed_us"]) / 1000
            for left, right in zip(presented, presented[1:])]
    span = ((presented[-1]["elapsed_us"] - presented[0]["elapsed_us"]) / 1_000_000
            if len(presented) > 1 else 0)
    rtts = [event["rtt_ms"] for event in diagnostics if event.get("rtt_ms") is not None]
    encode = [event["remote"]["capture"]["encode_us"] / 1000 for event in diagnostics
              if event.get("remote") and event["remote"].get("capture")]
    display = [event["local"]["receive_to_present_last_us"] / 1000 for event in diagnostics
               if event["local"].get("receive_to_present_last_us") is not None]
    all_notifications = sum(event.get("event") == "presented" for event in events)
    native = closed.get("native_summary") if closed else None
    observed = native.get("displayed") if native else None
    observation_ratio = (observed / all_notifications
                         if isinstance(observed, int) and all_notifications else None)
    def stats(values):
        return {"median": statistics.median(values) if values else None,
                "p95": percentile(values, .95), "max": max(values) if values else None}
    return {
        "input": str(path), "completed": opened is not None and closed is not None and not errors,
        "measurement_complete": measurement_complete, "telemetry": telemetry,
        # A native Present call/consumption notification does not prove scanout.
        # Sparse DXGI observations may reflect occlusion or missing telemetry;
        # either way they cannot substantiate the visible-performance gate.
        "native_observation_ratio": observation_ratio,
        "display_evidence_sufficient": observation_ratio is not None and observation_ratio >= .9,
        "opened": opened, "errors": errors, "warmup_seconds": warmup_seconds,
        "measured_present_span_seconds": span,
        "presentation_notifications": len(presented),
        # Watch notifications may coalesce. The native summary independently
        # records the renderer count; do not count frame-ID jumps as displays.
        "presentation_notification_fps": (len(presented) - 1) / span if span else None,
        "presentation_gap_ms": stats(gaps),
        "gaps_over_100ms": sum(gap > 100 for gap in gaps),
        "gaps_over_250ms": sum(gap > 250 for gap in gaps),
        "gaps_over_1s": sum(gap > 1000 for gap in gaps),
        "paths": sorted({event["path"] for event in diagnostics}),
        "decoders": sorted({event["decoder"] for event in presented if event.get("decoder")}),
        "rtt_ms": stats(rtts), "encode_ms": stats(encode),
        "receive_to_present_ms": stats(display),
        "capture_limits_fps": sorted({event["quality"]["capture_fps_milli"] / 1000
                                     for event in diagnostics if event.get("quality")}),
        "native_summary": native,
    }


def performance_failures(result):
    failures = []
    if not result["completed"] or not result["measurement_complete"]:
        failures.append("incomplete measurement")
    if not result["display_evidence_sufficient"]:
        failures.append("insufficient actual presentation observations")
    if (result["presentation_notification_fps"] or 0) < 28.5:
        failures.append("FPS below 28.5 gate")
    gaps = result["presentation_gap_ms"]
    if gaps["p95"] is None or gaps["p95"] > 67:
        failures.append("p95 gap above 67 ms")
    if gaps["max"] is None or gaps["max"] > 250:
        failures.append("maximum gap above 250 ms")
    if result["gaps_over_1s"]:
        failures.append("gap above one second")
    if result["paths"] != ["Some(Direct)"]:
        failures.append("not a direct LAN run")
    return failures


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--warmup-seconds", type=float, default=5)
    parser.add_argument("--check-performance", action="store_true")
    args = parser.parse_args()
    if args.warmup_seconds < 0:
        parser.error("warmup must be nonnegative")
    result = summarize(args.input, args.warmup_seconds)
    if args.check_performance:
        failures = performance_failures(result)
        result.update(status="failed" if failures else "passed", failures=failures)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"completed": result["completed"],
                      "presentation_notification_fps": result["presentation_notification_fps"],
                      "gaps_over_1s": result["gaps_over_1s"]}))
    if args.check_performance and failures:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
