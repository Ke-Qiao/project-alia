"""Use pinned official Easy-Turn architecture, preprocessing and generation."""

import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys

from .provenance import (
    CHECKPOINT_BYTES, CHECKPOINT_SHA256, MODEL_ID, MODEL_REVISION,
    QWEN_FILES, QWEN_ID, QWEN_REVISION, SOURCE_REVISION,
)

# The pinned checkpoint emits a literal EOS marker even with
# tokenizer.batch_decode(skip_special_tokens=True). Accept that exact suffix,
# observed in real official-sample inference, but never arbitrary trailing text.
LABEL = re.compile(r"<(complete|incomplete|backchannel|wait)>\s*(?:<\|endoftext\|>)?\s*$", re.IGNORECASE)


def parse_output(output) -> tuple[str, str]:
    if not isinstance(output, list) or len(output) != 1 or not isinstance(output[0], str):
        raise ValueError("Expected one official model output")
    match = LABEL.search(output[0])
    if not match:
        raise ValueError("The model output has no terminal turn label")
    transcript = output[0][:match.start()].strip()
    if re.search(r"<(complete|incomplete|backchannel|wait)>", transcript, re.IGNORECASE):
        raise ValueError("The model produced multiple turn labels")
    return transcript, match[1].lower()


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(8 * 1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def import_official_components(source_dir: Path):
    """Import only the verified local source, without its unused SoX initializer."""
    sys.dont_write_bytecode = True
    sys.path.insert(0, str(source_dir / "Easy_Turn"))
    import torchaudio
    from wenet.llm_asr import llmasr_model
    from wenet.transformer.encoder import TransformerEncoder

    # processor.py changes a process-global SoX file-decoder buffer on import.
    # Our strict raw-PCM path never uses SoX. Suppress just that initializer;
    # retain the official feature function and restore the binding immediately.
    original = torchaudio.utils.sox_utils.set_buffer_size
    try:
        torchaudio.utils.sox_utils.set_buffer_size = lambda _size: None
        from wenet.dataset.processor import compute_log_mel_spectrogram
    finally:
        torchaudio.utils.sox_utils.set_buffer_size = original
    return llmasr_model, TransformerEncoder, compute_log_mel_spectrogram


def verify_artifacts(source: Path, model_dir: Path, qwen_dir: Path):
    def git(*args):
        return subprocess.check_output(["git", "-C", str(source), *args], text=True).strip()

    if git("rev-parse", "HEAD") != SOURCE_REVISION:
        raise ValueError(f"Easy-Turn source must be at {SOURCE_REVISION}")
    if git("status", "--porcelain", "--untracked-files=normal", "--", "Easy_Turn"):
        raise ValueError("Easy-Turn inference source must be unmodified")
    checkpoint = model_dir / "checkpoint.pt"
    if checkpoint.stat().st_size != CHECKPOINT_BYTES or sha256(checkpoint) != CHECKPOINT_SHA256:
        raise ValueError("Easy-Turn checkpoint does not match the pinned official SHA-256")
    manifest = json.loads((qwen_dir / "alia-artifacts.json").read_text())
    if manifest.get("source") != QWEN_ID or manifest.get("revision") != QWEN_REVISION:
        raise ValueError("Qwen artifact provenance is missing or incompatible; run explicit prepare")
    for name in QWEN_FILES:
        if sha256(qwen_dir / name) != manifest.get("sha256", {}).get(name):
            raise ValueError(f"Qwen artifact checksum mismatch: {name}")
    config = json.loads((qwen_dir / "config.json").read_text())
    if config.get("model_type") != "qwen2" or config.get("auto_map"):
        raise ValueError("Only the pinned built-in Qwen2 architecture is supported")
    if any(qwen_dir.glob("*.py")):
        raise ValueError("Executable remote model files are not allowed")


class EasyTurnDetector:
    def __init__(self, source_dir: Path, model_dir: Path, qwen_dir: Path, device: str = "cpu"):
        source_dir, model_dir, qwen_dir = (p.resolve(strict=True) for p in (source_dir, model_dir, qwen_dir))
        verify_artifacts(source_dir, model_dir, qwen_dir)
        # Set before importing Hugging Face. All startup resources must already exist.
        os.environ["HF_HUB_OFFLINE"] = "1"
        os.environ["TRANSFORMERS_OFFLINE"] = "1"
        os.environ["HF_HUB_DISABLE_TELEMETRY"] = "1"
        os.environ.setdefault("HF_HOME", str(model_dir / ".cache/huggingface"))

        import numpy as np
        import torch
        import yaml
        from transformers import AutoModelForCausalLM, AutoTokenizer
        llmasr_model, TransformerEncoder, compute_log_mel_spectrogram = import_official_components(source_dir)

        if device == "auto":
            device = "cpu"  # MPS must be explicitly selected until target-device acceptance.
        if device not in ("cpu", "mps"):
            raise ValueError("device must be cpu, mps, or auto")
        if device == "mps" and not torch.backends.mps.is_available():
            raise ValueError("MPS was requested but is unavailable; no silent CPU fallback")
        if device == "mps":
            if os.environ.get("PYTORCH_ENABLE_MPS_FALLBACK") == "1":
                raise ValueError("Unset PYTORCH_ENABLE_MPS_FALLBACK or select cpu; mixed fallback is not validated")
            try:
                # Transformers uses this exact token-membership operation before
                # generation. Torch 2.2.2/MPS fails it on the validation Mac.
                torch.isin(torch.tensor([151643], device="mps"),
                           torch.tensor([-100], device="mps")).any().item()
            except (NotImplementedError, RuntimeError) as exc:
                raise ValueError(
                    "MPS cannot execute the required torch.isin token operation with this runtime; "
                    "use --device cpu. A new MPS dependency stack needs separate real-model acceptance."
                ) from exc

        # The pinned upstream constructor hardcodes BF16 and remote-code trust.
        # Override only its two loader bindings, retaining official model/generate.
        class LocalModelLoader:
            @staticmethod
            def from_pretrained(path, **kwargs):
                kwargs.update(torch_dtype=torch.float32, local_files_only=True,
                              trust_remote_code=False, attn_implementation="eager")
                return AutoModelForCausalLM.from_pretrained(path, **kwargs)

        class LocalTokenizerLoader:
            @staticmethod
            def from_pretrained(path, **kwargs):
                kwargs.update(local_files_only=True, trust_remote_code=False)
                return AutoTokenizer.from_pretrained(path, **kwargs)

        config_root = source_dir / "Easy_Turn/examples/wenetspeech/whisper/conf"
        configs = yaml.safe_load((config_root / "train.yaml").read_text())
        old_model, old_tokenizer = llmasr_model.AutoModelForCausalLM, llmasr_model.AutoTokenizer
        llmasr_model.AutoModelForCausalLM = LocalModelLoader
        llmasr_model.AutoTokenizer = LocalTokenizerLoader
        try:
            encoder = TransformerEncoder(configs["input_dim"], global_cmvn=None, **configs["encoder_conf"])
            self.model = llmasr_model.LLMASR_Model(
                encoder=encoder, encoder_output_dim=configs["encoder_conf"]["output_size"],
                llm_path=str(qwen_dir), lora=configs["use_lora"],
                lora_alpha=configs["lora_alpha"], lora_rank=configs["lora_rank"],
                lora_dropout=configs["lora_dropout"], is_inference=True,
                downsample_rate=configs["downsample_rate"], adapter_type=configs["adapter_type"],
                speech_token_num=configs["speech_token_num"], train_speech_out=True,
            ).float()
        finally:
            llmasr_model.AutoModelForCausalLM, llmasr_model.AutoTokenizer = old_model, old_tokenizer
        # Upstream logs missing weights and continues. A resident service must fail
        # startup instead of serving predictions from partly random parameters.
        weights = torch.load(model_dir / "checkpoint.pt", map_location="cpu", weights_only=True)
        self.model.load_state_dict(weights, strict=True)
        del weights
        self.model.to(torch.device(device)).eval().requires_grad_(False)
        self.prompt = yaml.safe_load((config_root / "prompt.yaml").read_text())[
            "<TRANSCRIBE> <BACKCHANNEL> <COMPLETE>"
        ][0]
        self.features = compute_log_mel_spectrogram
        self.feature_options = configs["dataset_conf"]["log_mel_spectrogram_conf"]
        self.torch, self.np, self.device = torch, np, device
        self.metadata = {
            "source": MODEL_ID, "revision": MODEL_REVISION, "source_revision": SOURCE_REVISION,
            "qwen_revision": QWEN_REVISION, "device": device, "dtype": "float32",
        }

    def detect(self, pcm: bytes) -> tuple[str, str]:
        torch = self.torch
        waveform = torch.from_numpy(self.np.frombuffer(pcm, dtype="<i2").astype("float32") / 32768.0)
        sample = {"sample_rate": 16000, "wav": waveform.unsqueeze(0), "key": "window", "label": []}
        # The official CPU STFT/librosa path avoids MPS complex-STFT limitations.
        with torch.inference_mode():
            features = next(self.features(iter([sample]), **self.feature_options))["feat"]
            wavs = features.unsqueeze(0).to(self.device)
            lengths = torch.tensor([features.shape[0]], device=self.device)
            output = self.model.generate(wavs=wavs, wavs_len=lengths, prompt=self.prompt)
        return parse_output(output)
