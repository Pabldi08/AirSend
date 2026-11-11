import unittest
from types import SimpleNamespace

import numpy as np

from calibrate import chirp, detect, measure, smooth_capture_clock


class DetectionTests(unittest.TestCase):
    rate = 16000

    def signal(self, delays=(2.15,), gains=(0.8,), noise=0.001):
        ref = chirp(self.rate)
        data = np.random.default_rng(41).normal(0, noise, 4 * self.rate)
        for delay, gain in zip(delays, gains):
            start = round((0.5 + delay) * self.rate)
            data[start:start + len(ref)] += gain * ref
        # Absolute device time can be arbitrary, and ADC/DAC origins differ.
        times = 123.0 + np.arange(len(data)) / self.rate
        return data, times, ref

    def test_recovers_known_delay_with_noise_and_inverted_polarity(self):
        data, times, ref = self.signal(gains=(-0.3,))
        result = detect(data, times, ref, 123.5, self.rate, 3)
        self.assertAlmostEqual(result["delay_ms"], 2150, delta=0.1)
        self.assertGreater(result["correlation"], 0.8)

    def test_rejects_monitor_and_homepod_arriving_separately(self):
        data, times, ref = self.signal((0.02, 2.15), (0.7, 0.8))
        with self.assertRaisesRegex(ValueError, "Multiple arrivals"):
            detect(data, times, ref, 123.5, self.rate, 3)

    def test_rejects_noise_only(self):
        data, times, ref = self.signal((), ())
        with self.assertRaisesRegex(ValueError, "No clear chirp"):
            detect(data, times, ref, 123.5, self.rate, 3)

    def test_ignores_signal_outside_requested_search_window(self):
        data, times, ref = self.signal()
        with self.assertRaisesRegex(ValueError, "No clear chirp"):
            detect(data, times, ref, 123.5, self.rate, 1)

    def test_silence_has_no_nan_or_false_detection(self):
        data, times, ref = self.signal()
        with self.assertRaisesRegex(ValueError, "No clear chirp"):
            detect(np.zeros_like(data), times, ref, 123.5, self.rate, 3)

    def test_low_latency_arrival(self):
        data, times, ref = self.signal((0.035,), (0.8,))
        result = detect(data, times, ref, 123.5, self.rate, 3)
        self.assertAlmostEqual(result["delay_ms"], 35, delta=0.1)


class FakeAudio:
    """Exercise duplex timing/cleanup without accessing a real microphone."""

    class CallbackStop(Exception):
        pass

    class CallbackAbort(Exception):
        pass

    def __init__(self, status=False, clipped=False):
        self.status = status
        self.clipped = clipped
        self.closed = False
        self.output = []

    def query_devices(self, device, kind):
        return {"hostapi": 0, "max_output_channels": 2}

    def query_hostapis(self, device):
        return {"name": "Windows WASAPI"}

    def check_input_settings(self, **kwargs):
        pass

    def check_output_settings(self, **kwargs):
        pass

    def WasapiSettings(self, **kwargs):
        return SimpleNamespace(**kwargs)

    def Stream(self, **kwargs):
        audio = self
        rate = kwargs["samplerate"]
        # ADC time and DAC time differ by 50 ms. Simulated acoustic path
        # delay is 350 ms, so recorded chirps occur 400 ms after output indices.
        recording = np.zeros(7 * rate + 2048, dtype=np.float32)
        for trial in range(3):
            reference = chirp(rate, trial)
            start = round((1 + 2 * trial + 0.4) * rate)
            recording[start:start + len(reference)] = reference
        if self.clipped:
            recording[0] = 1.0

        class Stream:
            latency = (0.02, 0.03)

            def __enter__(self):
                position = 0
                try:
                    while True:
                        frames = min(1024, len(recording) - position)
                        if frames <= 0:
                            raise AssertionError("Callback did not stop")
                        out = np.empty((frames, 2), dtype=np.float32)
                        timing = SimpleNamespace(inputBufferAdcTime=20 + position / rate,
                                                 outputBufferDacTime=20.05 + position / rate)
                        try:
                            kwargs["callback"](recording[position:position + frames, None],
                                               out, frames, timing, audio.status)
                        finally:
                            audio.output.append(out.copy())
                        position += frames
                except FakeAudio.CallbackStop:
                    kwargs["finished_callback"]()
                return self

            def __exit__(self, *args):
                audio.closed = True

        return Stream()


class CaptureTests(unittest.TestCase):
    def test_small_callback_clock_jitter_is_smoothed_and_reported(self):
        rate = 16000
        expected = 50 + np.arange(2 * rate) / rate
        times = expected + 0.002 * np.sin(np.arange(2 * rate) / 123)
        fitted, jitter = smooth_capture_clock(times, rate)
        self.assertTrue(np.all(np.diff(fitted) > 0))
        self.assertLess(np.max(np.abs(fitted - expected)), 0.0005)
        self.assertGreater(jitter, 1)

    def test_large_clock_discontinuity_is_rejected(self):
        rate = 16000
        times = 50 + np.arange(2 * rate) / rate
        times[rate:] += 0.1
        with self.assertRaisesRegex(ValueError, "Unstable device clock"):
            smooth_capture_clock(times, rate)

    def test_duplex_timestamps_correct_for_adc_dac_offset_and_close_stream(self):
        audio = FakeAudio()
        report = measure(audio, 0, 1, 16000, 3, 1)
        self.assertAlmostEqual(report["median_delay_ms"], 350, delta=0.1)
        self.assertLess(report["spread_ms"], 0.1)
        self.assertTrue(audio.closed)
        emitted = np.concatenate(audio.output)[:, 0]
        self.assertTrue(np.all(emitted[:16000] == 0))
        self.assertLessEqual(np.max(np.abs(emitted)), 0.12)

    def test_discontinuity_discards_measurement_and_closes_stream(self):
        audio = FakeAudio(status=True)
        with self.assertRaisesRegex(ValueError, "discontinuity"):
            measure(audio, 0, 1, 16000, 3, 1)
        self.assertTrue(audio.closed)

    def test_clipped_microphone_discards_measurement(self):
        audio = FakeAudio(clipped=True)
        with self.assertRaisesRegex(ValueError, "clipped"):
            measure(audio, 0, 1, 16000, 3, 1)
        self.assertTrue(audio.closed)


if __name__ == "__main__":
    unittest.main()
