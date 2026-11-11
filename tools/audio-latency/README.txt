AirSend acoustic latency calibration (Windows helper)

This helper measures the actual route Windows output -> AirSend capture ->
AirPlay -> HomePod speaker -> physical microphone. Run it with Windows Python,
not WSL Python. It requires AirSend to be installed/running and already streaming.
It is a standalone diagnostic, not yet an AirSend UI feature.

Setup from this directory in PowerShell:

  py -3 -m venv "$env:LOCALAPPDATA\AirSend\calibration-venv"
  & "$env:LOCALAPPDATA\AirSend\calibration-venv\Scripts\python.exe" -m pip install -r requirements.txt
  & "$env:LOCALAPPDATA\AirSend\calibration-venv\Scripts\python.exe" calibrate.py --list-devices

After setup, run.cmd forwards its arguments to this isolated Windows Python:

  .\run.cmd --list-devices

Choose a physical microphone INPUT ID and the OUTPUT ID AirSend captures.
Use the Windows WASAPI entries (each device can appear under multiple APIs).
The microphone must not be Stereo Mix, a loopback device, or a virtual microphone.
Shared-mode WASAPI conversion handles different USB/Bluetooth mixer rates;
the working sample rate defaults to 48000 (try --rate 44100 if unsupported).
Allow Windows desktop microphone access if needed.

Before measuring:

1. Start AirSend streaming to ONE HomePod. Pause other audio.
2. Ensure the monitor/local speakers cannot be heard. Prefer an installed virtual
   render output or a hardware-muted output that still provides loopback samples.
   AirSend's experimental local mute can cut capture on some drivers; verify the
   HomePod still plays. Do not use Windows microphone "Listen to this device".
3. Place the physical microphone near the HomePod. Disable microphone echo
   cancellation/noise suppression if available; a headset mic works too.
4. Replace IDs below with actual device IDs from --list-devices:

  & "$env:LOCALAPPDATA\AirSend\calibration-venv\Scripts\python.exe" calibrate.py --input 12 --output 15 --local-output-silent --report baseline.json

Three different 80 ms chirps play, spaced six seconds apart (about 19 seconds
total). The microphone is recorded only during this run and stays in memory.
The report contains numbers, not audio or device names. Ctrl+C closes the streams.
Start with a comfortable HomePod volume; the generated waveform peaks at 12%.

Compare the default AirSend settings with reduced local buffers enabled and
receiver buffer settings such as 1000, 300, and 100 ms. Confirm each setting in
AirSend and wait until streaming has resumed before running another measurement.
Use different report filenames; existing files are never overwritten. Record
the settings alongside each report. Also listen for dropouts during longer audio;
a low chirp delay alone does not establish stable playback.

Interpretation:

- median_delay_ms estimates output-to-microphone delay using device-reported
  playback timing and a smoothed capture sample clock. It includes AirSend
  capture/encoding/network/receiver delay and acoustic travel.
- spread_ms shows variation between chirps. Correlation is a waveform match,
  not a statistical confidence interval. Room echoes and microphone processing
  can bias arrival detection. Some WASAPI drivers return callback timing in place
  of true ADC/DAC timing, introducing an uncalibrated bias. capture_clock_jitter_ms
  quantifies observed callback clock variation, not the total measurement error.
  Hardware timestamps are not independently calibrated;
  sample resolution is not measurement accuracy. This does not measure video sync.
- Silence, clipping, audio overruns/underruns, large timestamp gaps, or multiple strong
  arrivals fail the run rather than produce an apparently valid latency number.
  Quiet room echoes may remain; repeat with the mic closer to the HomePod.
- This measures delay; it does not automatically compensate or change settings.

Development tests (no audio devices or microphone required):

  python -m unittest discover -s tools/audio-latency -v
