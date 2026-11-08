#!/usr/bin/env python3
"""Neural voice sidecar for Aussie Accent Coach (Chatterbox, MIT licence).

The app starts this once and keeps it running so the model stays loaded. Protocol:
one JSON object per line on stdin, one JSON reply per line on stdout.

    -> {"text": "G'day mate.", "voice": "/path/clip.wav", "out": "/tmp/x.wav"}
    <- {"ok": true}          or   {"error": "..."}

On start it prints {"ready": true, "device": "mps"} once the model has loaded.
The output is 16 kHz mono WAV. The accent comes from the voice clip, so give it an
Australian speaker.
"""

import json
import sys


def main() -> int:
    # Libraries print progress to stdout; keep the protocol channel clean.
    proto = sys.stdout
    sys.stdout = sys.stderr

    def reply(obj):
        proto.write(json.dumps(obj) + "\n")
        proto.flush()

    try:
        import torch
        import torchaudio
        from chatterbox.tts import ChatterboxTTS
    except ImportError as e:
        reply({"error": f"missing package {e.name}; run scripts/setup_voice_model.py"})
        return 1

    device = "mps" if torch.backends.mps.is_available() else "cpu"
    try:
        model = ChatterboxTTS.from_pretrained(device=device)
    except Exception as e:  # MPS can fail on some torch builds; CPU always works.
        print(f"{device} failed ({e}); falling back to CPU", file=sys.stderr)
        device = "cpu"
        model = ChatterboxTTS.from_pretrained(device=device)
    reply({"ready": True, "device": device})

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
            with torch.inference_mode():
                wav = model.generate(
                    req["text"],
                    audio_prompt_path=req["voice"],
                    exaggeration=float(req.get("exaggeration", 0.5)),
                    cfg_weight=float(req.get("cfg_weight", 0.5)),
                )
            wav = torchaudio.functional.resample(wav.cpu(), model.sr, 16000)
            torchaudio.save(req["out"], wav, 16000, encoding="PCM_S", bits_per_sample=16)
            reply({"ok": True})
        except Exception as e:
            reply({"error": str(e)})
    return 0


if __name__ == "__main__":
    sys.exit(main())
