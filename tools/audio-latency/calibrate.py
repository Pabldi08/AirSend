#!/usr/bin/env python3
"""Measure Windows output -> AirSend -> HomePod -> physical microphone delay.

Run --list-devices first, then --help. Audio stays in memory; only numerical
results are saved when --report is supplied. This does not change app settings.
"""

import argparse
import json
import sys
import threading
from pathlib import Path

import numpy as np


def chirp(rate, trial=0):
    """A tapered, distinct 80 ms sweep per trial, at a modest digital level."""
    t = np.arange(round(rate * 0.08), dtype=np.float64) / rate
    start = 1200 + 350 * trial
    phase = 2 * np.pi * (start * t + (4200 - start) * t * t / 0.16)
    return (0.12 * np.sin(phase) * np.hanning(len(t))).astype(np.float32)


def scores(recording, reference):
    """Normalized matched filter using FFT; no quadratic full-recording scan."""
    data = np.asarray(recording, dtype=np.float64)
    ref = np.asarray(reference, dtype=np.float64)
    if len(data) < len(ref):
        raise ValueError("Recording shorter than reference")
    size = 1 << (len(data) + len(ref) - 2).bit_length()
    convolution = np.fft.irfft(
        np.fft.rfft(data, size) * np.fft.rfft(ref[::-1], size), size
    )
    dot = convolution[len(ref) - 1 : len(data)]
    energy = np.concatenate(([0.0], np.cumsum(data * data)))
    norm = np.sqrt(np.maximum(0, energy[len(ref):] - energy[:-len(ref)]) * (ref @ ref))
    return np.abs(dot) / np.maximum(norm, 1e-12)


def detect(recording, sample_times, reference, sent_at, rate, max_delay):
    """Find arrival by ADC/DAC timestamps; reject missing/ambiguous signals."""
    correlation = scores(recording, reference)
    times = sample_times[:len(correlation)]
    valid = (times >= sent_at) & (times <= sent_at + max_delay)
    candidates = np.where(valid, correlation, 0.0)
    peak = int(np.argmax(candidates))
    confidence = float(candidates[peak])
    if confidence < 0.25:
        raise ValueError(f"No clear chirp detected (best correlation {confidence:.3f}): "
                         "check routing, microphone, and HomePod volume")
    # A direct monitor sound and a delayed HomePod sound must not be silently
    # collapsed to whichever peak happens to be loudest.
    distant = np.abs(times - times[peak]) > 0.08
    if np.max(np.where(valid & distant, correlation, 0.0)) >= confidence * 0.70:
        raise ValueError("Multiple arrivals: silence the monitor and move the mic nearer the HomePod")
    return {"delay_ms": float((times[peak] - sent_at) * 1000),
            "correlation": confidence, "sample_resolution_ms": 1000 / rate}


def smooth_capture_clock(sample_times, rate):
    """Fit the continuous captured sample clock despite callback timestamp jitter.

    WASAPI's callback timestamps can overlap by a few milliseconds even without
    dropped samples. Keep that observed uncertainty visible in the report.
    Large timing deviations still invalidate the run.
    """
    offsets = np.arange(len(sample_times), dtype=np.float64) / rate
    probes = (sample_times - offsets)[rate // 4::max(1, rate // 100)]
    if len(probes) == 0 or not np.all(np.isfinite(probes)) or np.any(probes <= 0):
        raise ValueError("Invalid device timestamps; measurement discarded")
    origin = float(np.median(probes))
    jitter_ms = float(np.max(np.abs(probes - origin)) * 1000)
    if jitter_ms > 25:
        raise ValueError(f"Unstable device clock ({jitter_ms:.1f} ms); measurement discarded")
    return origin + offsets, jitter_ms


def measure(sd, input_device, output_device, rate, trials, max_delay):
    """Full duplex callback: no microphone monitoring and no disk/network I/O."""
    interval = max_delay + 1
    starts = [round((1 + i * interval) * rate) for i in range(trials)]
    total = round((1 + trials * interval) * rate)
    outgoing = np.zeros(total, dtype=np.float32)
    references = [chirp(rate, i) for i in range(trials)]
    for start, reference in zip(starts, references):
        outgoing[start:start + len(reference)] = reference
    incoming = np.zeros(total, dtype=np.float32)
    sample_times = np.zeros(total, dtype=np.float64)
    # Preallocation keeps recording allocation/correlation out of the callback.
    offsets = np.arange(total, dtype=np.float64) / rate
    sent = np.full(trials, np.nan)
    position = 0
    issues = []
    done = threading.Event()
    failure = []

    def callback(indata, outdata, frames, time_info, status):
        nonlocal position
        outdata.fill(0)
        try:
            if status:
                issues.append(str(status))
            count = min(frames, total - position)
            end = position + count
            incoming[position:end] = indata[:count, 0]
            np.add(offsets[:count], time_info.inputBufferAdcTime,
                   out=sample_times[position:end])
            outdata[:count, :] = outgoing[position:end, None]
            for i, start in enumerate(starts):
                if position <= start < end:
                    sent[i] = time_info.outputBufferDacTime + (start - position) / rate
            position = end
        except Exception as error:
            failure.append(str(error))
            raise sd.CallbackAbort from error
        if position == total:
            raise sd.CallbackStop

    info_in = sd.query_devices(input_device, "input")
    info_out = sd.query_devices(output_device, "output")
    if info_in["hostapi"] != info_out["hostapi"]:
        raise ValueError("Choose input and output from the same host API (Windows WASAPI)")
    host = sd.query_hostapis(info_in["hostapi"])["name"]
    if sys.platform == "win32" and host != "Windows WASAPI":
        raise ValueError("Choose Windows WASAPI device IDs for consistent ADC/DAC timing")
    # Bluetooth render and USB input often have different mixer rates. Shared
    # mode conversion preserves the selected endpoints without changing Windows.
    extra = sd.WasapiSettings(auto_convert=True) if host == "Windows WASAPI" else None
    channels = (1, min(2, int(info_out["max_output_channels"])))
    sd.check_input_settings(device=input_device, channels=1, samplerate=rate,
                            dtype="float32", extra_settings=extra)
    sd.check_output_settings(device=output_device, channels=channels[1], samplerate=rate,
                             dtype="float32", extra_settings=extra)
    with sd.Stream(device=(input_device, output_device), samplerate=rate,
                   channels=channels, dtype="float32", blocksize=0, latency="high",
                   callback=callback, finished_callback=done.set,
                   extra_settings=extra) as stream:
        latencies = list(stream.latency)
        if not done.wait(total / rate + 10):
            raise ValueError("Audio device timed out; measurement discarded")
    if failure or issues or position != total:
        raise ValueError(f"Audio capture discontinuity; measurement discarded: {failure or issues}")
    if not np.all(np.isfinite(sent)):
        raise ValueError("Invalid device timestamps; measurement discarded")
    sample_times, clock_jitter_ms = smooth_capture_clock(sample_times, rate)
    if np.max(np.abs(incoming)) >= 0.99:
        raise ValueError("Microphone clipped; reduce gain/volume or move it farther away")
    results = [detect(incoming, sample_times, ref, at, rate, max_delay)
               for ref, at in zip(references, sent)]
    delays = [r["delay_ms"] for r in results]
    return {
        "schema_version": 1,
        "measurement": "Estimated Windows output to microphone arrival; includes AirSend, receiver, and acoustic path",
        "limitations": "Uses device-reported playback timestamps and a smoothed capture sample clock. Some WASAPI drivers report callback timing instead of true ADC/DAC timing. Device timestamp bias is uncalibrated; room echoes and microphone processing can also bias results. Sample resolution is not measurement accuracy.",
        "sample_rate": rate, "trials": results,
        "shared_mode_conversion": host == "Windows WASAPI",
        "median_delay_ms": float(np.median(delays)),
        "spread_ms": float(np.ptp(delays)),
        "capture_clock_jitter_ms": clock_jitter_ms,
        "reported_input_output_latency_ms": [v * 1000 for v in latencies],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--list-devices", action="store_true")
    parser.add_argument("--input", type=int, help="Physical microphone device ID")
    parser.add_argument("--output", type=int, help="Windows output ID that AirSend is capturing")
    parser.add_argument("--local-output-silent", action="store_true",
                        help="Confirm that only the HomePod will be audible")
    parser.add_argument("--rate", type=int, choices=[44100, 48000], default=48000)
    parser.add_argument("--trials", type=int, choices=range(3, 6), default=3)
    parser.add_argument("--max-delay", type=float, default=5.0)
    parser.add_argument("--report", type=Path, help="Save numerical JSON report; never saves audio")
    args = parser.parse_args()
    import sounddevice as sd
    if args.list_devices:
        print(sd.query_devices())
        return
    if args.input is None or args.output is None or not args.local_output_silent:
        parser.error("Select --input and --output and confirm --local-output-silent; see README.txt")
    if not 0.1 <= args.max_delay <= 10:
        parser.error("--max-delay must be between 0.1 and 10 seconds")
    print("Start AirSend streaming first. Recording chirps locally; Ctrl+C cancels.", flush=True)
    result = measure(sd, args.input, args.output, args.rate, args.trials, args.max_delay)
    payload = json.dumps(result, indent=2, allow_nan=False)
    print(payload)
    if args.report:
        # Avoid overwriting a previous measurement during repeated comparisons.
        with args.report.open("x", encoding="utf-8") as report:
            report.write(payload + "\n")


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("Cancelled; audio devices closed.", file=sys.stderr)
        sys.exit(130)
    except Exception as error:
        print(f"Calibration failed: {error}", file=sys.stderr)
        sys.exit(1)
