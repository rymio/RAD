#!/usr/bin/env python3
"""Install the neural voice model (Chatterbox, MIT licence) for Aussie Accent Coach.

Run once on your Mac (Chatterbox needs Python 3.11):

    python3.11 -m venv .venv-voice && source .venv-voice/bin/activate
    pip install chatterbox-tts "setuptools<81"
    python3 scripts/setup_voice_model.py

This downloads the model weights (~2 GB, cached by Hugging Face), checks that speech can
be generated, and writes into ~/Library/Application Support/AussieAccentCoach/models/:

    chatterbox/voice_server.py   the sidecar the app runs
    chatterbox.json              which Python to run it with (this venv's)

Chatterbox copies the voice and accent of a short clip, so in the app add a 5-20 second
recording of an Australian speaker (with their permission) as a neural voice.
"""

import argparse
import json
import shutil
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent


def models_dir() -> Path:
    return Path.home() / "Library" / "Application Support" / "AussieAccentCoach" / "models"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--out", type=Path, default=models_dir(), help="models folder")
    ap.add_argument("--skip-test", action="store_true", help="do not generate a test sentence")
    args = ap.parse_args()

    try:
        import pkg_resources  # noqa: F401  (the watermarker needs it; setuptools 81+ dropped it)
    except ImportError:
        print('Run: pip install "setuptools<81"  (Chatterbox\'s watermarker needs pkg_resources)', file=sys.stderr)
        return 1
    try:
        import torch
        import torchaudio
        from chatterbox.tts import ChatterboxTTS
    except ImportError as e:
        print(f"Missing package: {e.name}. Run: pip install chatterbox-tts", file=sys.stderr)
        return 1

    device = "mps" if torch.backends.mps.is_available() else "cpu"
    print(f"Downloading Chatterbox weights and loading on {device} …")
    try:
        model = ChatterboxTTS.from_pretrained(device=device)
    except Exception as e:
        print(f"{device} failed ({e}); trying CPU")
        device = "cpu"
        model = ChatterboxTTS.from_pretrained(device=device)

    if not args.skip_test:
        print("Generating a test sentence …")
        wav = model.generate("G'day, this is a test of the neural voice.")
        out = Path(tempfile.gettempdir()) / "aussie-coach-voice-test.wav"
        torchaudio.save(str(out), wav.cpu(), model.sr)
        print(f"OK: {out} (default voice; the app uses your Australian clip instead)")

    server_dir = args.out / "chatterbox"
    server_dir.mkdir(parents=True, exist_ok=True)
    shutil.copy2(HERE / "voice_server.py", server_dir / "voice_server.py")
    (args.out / "chatterbox.json").write_text(
        json.dumps({"python": sys.executable, "server": str(server_dir / "voice_server.py"), "device": device}, indent=2)
    )
    print(f"Installed into {args.out}. In the app choose Models › Reload speech models, then add a neural voice.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
