"""
Modal training infrastructure for cad0 model.

LoRA fine-tuning of Qwen 2.5-Coder 7B on VCode generation.
"""

from .config import ModelConfig, TrainingConfig, DataConfig
from .data import load_dataset, format_for_training, create_data_collator
from .train import train_model
from .eval import evaluate_model, compute_metrics

__all__ = [
    "ModelConfig",
    "TrainingConfig",
    "DataConfig",
    "load_dataset",
    "format_for_training",
    "create_data_collator",
    "train_model",
    "evaluate_model",
    "compute_metrics",
]
