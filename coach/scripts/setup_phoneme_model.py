#!/usr/bin/env python3
"""Export the wav2vec2 phoneme model to ONNX for Aussie Accent Coach (milestone M2).

Run once on your Mac:

    python3 -m venv .venv && source .venv/bin/activate
    pip install torch transformers onnx onnxruntime
    python3 scripts/setup_phoneme_model.py            # int8-quantised, ~330 MB
    python3 scripts/setup_phoneme_model.py --full     # float32, ~1.2 GB, slightly more accurate

It writes three files into ~/Library/Application Support/AussieAccentCoach/models/:

    wav2vec2-phonemes.onnx         the acoustic model (16 kHz audio -> phone logits, 20 ms frames)
    wav2vec2-phonemes.vocab.json   token table; "<pad>" is the CTC blank
    libonnxruntime.dylib           the ONNX Runtime from the onnxruntime wheel, loaded by the app

The model is facebook/wav2vec2-lv-60-espeak-cv-ft (Apache-2.0), fine-tuned to output
espeak-style IPA phones. The app never needs a pronunciation dictionary: it reads the
expected phones off the Australian reference recording with this same model.
"""

import argparse
import glob
import json
import os
import shutil
import sys
from pathlib import Path

DEFAULT_MODEL = "facebook/wav2vec2-lv-60-espeak-cv-ft"
MODEL_FILE = "wav2vec2-phonemes.onnx"
VOCAB_FILE = "wav2vec2-phonemes.vocab.json"


def models_dir() -> Path:
    return Path.home() / "Library" / "Application Support" / "AussieAccentCoach" / "models"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--model", default=DEFAULT_MODEL, help="Hugging Face model id (a wav2vec2 phoneme CTC model)")
    ap.add_argument("--out", type=Path, default=models_dir(), help="output folder")
    ap.add_argument("--full", action="store_true", help="keep float32 weights (no int8 quantisation)")
    args = ap.parse_args()

    try:
        import torch
        from transformers import AutoProcessor, Wav2Vec2ForCTC
        import onnxruntime
    except ImportError as e:
        print(f"Missing package: {e.name}. Run: pip install torch transformers onnx onnxruntime", file=sys.stderr)
        return 1

    args.out.mkdir(parents=True, exist_ok=True)
    print(f"Downloading {args.model} …")
    processor = AutoProcessor.from_pretrained(args.model)
    model = Wav2Vec2ForCTC.from_pretrained(args.model).eval()

    vocab = processor.tokenizer.get_vocab()
    if "<pad>" not in vocab:
        print("This model's vocabulary has no <pad> token (CTC blank); pick a wav2vec2 CTC model.", file=sys.stderr)
        return 1
    (args.out / VOCAB_FILE).write_text(json.dumps(vocab, ensure_ascii=False, indent=0), encoding="utf-8")
    print(f"Wrote {VOCAB_FILE} ({len(vocab)} tokens)")

    fp32_path = args.out / ("wav2vec2-phonemes.fp32.onnx" if not args.full else MODEL_FILE)
    dummy = torch.zeros(1, 16000)
    print("Exporting to ONNX …")
    with torch.no_grad():
        torch.onnx.export(
            model,
            (dummy,),
            str(fp32_path),
            input_names=["input_values"],
            output_names=["logits"],
            dynamic_axes={"input_values": {1: "samples"}, "logits": {1: "frames"}},
            opset_version=17,
            dynamo=False,
        )

    if not args.full:
        from onnxruntime.quantization import QuantType, quantize_dynamic

        print("Quantising to int8 …")
        quantize_dynamic(str(fp32_path), str(args.out / MODEL_FILE), weight_type=QuantType.QInt8)
        fp32_path.unlink()
        for extra in glob.glob(str(args.out / "wav2vec2-phonemes.fp32.onnx*")):
            os.remove(extra)
    print(f"Wrote {MODEL_FILE} ({(args.out / MODEL_FILE).stat().st_size / 1e6:.0f} MB)")

    # Copy the ONNX Runtime library from the wheel so the app can load the same version.
    capi = Path(onnxruntime.__file__).parent / "capi"
    libs = sorted(capi.glob("libonnxruntime*.dylib")) + sorted(capi.glob("libonnxruntime.so*"))
    if libs:
        target = args.out / ("libonnxruntime.dylib" if sys.platform == "darwin" else "libonnxruntime.so")
        shutil.copy2(libs[0], target)
        print(f"Copied {libs[0].name} -> {target.name} (onnxruntime {onnxruntime.__version__})")
    else:
        print("Could not find libonnxruntime in the onnxruntime wheel; install it with `brew install onnxruntime`.")

    # Sanity check: run one second of silence through the exported model.
    sess = onnxruntime.InferenceSession(str(args.out / MODEL_FILE))
    import numpy as np

    out = sess.run(None, {"input_values": np.zeros((1, 16000), dtype=np.float32)})[0]
    assert out.shape[2] == len(vocab), f"logits have {out.shape[2]} classes, vocab has {len(vocab)}"
    print(f"Check passed: 1 s of audio -> {out.shape[1]} frames x {out.shape[2]} classes")
    print(f"Done. Restart Aussie Accent Coach to use pronunciation scoring. Files are in {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
