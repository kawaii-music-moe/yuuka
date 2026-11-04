"""Shared loader for the v4.1 ONNX export (model only, no codec/watermark)."""
from __future__ import annotations

from pathlib import Path

from irodori_tts.config import ModelConfig, merge_dataclass_overrides
from irodori_tts.inference_runtime import _load_checkpoint_for_inference, download_hf_checkpoint
from irodori_tts.model import TextToLatentRFDiT

DEFAULT_REPO = "Aratako/Irodori-TTS-v4.1-Small-MF"


def load_model(repo: str = DEFAULT_REPO):
    ckpt = Path(download_hf_checkpoint(repo))
    state, cfg_dict, train_cfg, text_encoder_config = _load_checkpoint_for_inference(ckpt)
    cfg = merge_dataclass_overrides(ModelConfig(), cfg_dict, section="checkpoint model_config")
    model = TextToLatentRFDiT(
        cfg,
        pretrained_backbone_config=text_encoder_config,
        load_pretrained_backbone_weights=not cfg.use_pretrained_text_encoder,
    )
    model.load_state_dict(state, assign=cfg.use_pretrained_text_encoder)
    model.eval()
    return model, cfg, (train_cfg if isinstance(train_cfg, dict) else {}), ckpt
