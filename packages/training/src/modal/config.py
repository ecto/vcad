"""
Training configuration for cad0 model.
"""

from dataclasses import dataclass, field
from typing import Optional


@dataclass
class ModelConfig:
    """Configuration for the base model and LoRA."""

    # Base model
    model_name: str = "Qwen/Qwen2.5-Coder-7B"

    # LoRA configuration
    lora_r: int = 64  # Higher rank for code generation
    lora_alpha: int = 128  # 2x rank
    lora_dropout: float = 0.05
    lora_target_modules: list[str] = field(
        default_factory=lambda: [
            "q_proj",
            "k_proj",
            "v_proj",
            "o_proj",
            "gate_proj",
            "up_proj",
            "down_proj",
        ]
    )

    # Quantization
    use_4bit: bool = True
    bnb_4bit_compute_dtype: str = "bfloat16"
    bnb_4bit_quant_type: str = "nf4"
    use_nested_quant: bool = False

    # Attention
    use_flash_attention: bool = True


@dataclass
class TrainingConfig:
    """Configuration for training loop."""

    # Output
    output_dir: str = "/data/checkpoints"
    run_name: str = "cad0-qwen-7b-lora"

    # Training hyperparameters
    num_train_epochs: int = 3
    per_device_train_batch_size: int = 4
    per_device_eval_batch_size: int = 4
    gradient_accumulation_steps: int = 8  # Effective batch size = 32
    learning_rate: float = 2e-4
    weight_decay: float = 0.01
    warmup_ratio: float = 0.03
    lr_scheduler_type: str = "cosine"
    max_grad_norm: float = 0.3

    # Sequence length
    max_seq_length: int = 1024

    # Optimization
    optim: str = "paged_adamw_32bit"
    gradient_checkpointing: bool = True
    bf16: bool = True
    tf32: bool = True

    # Logging
    logging_steps: int = 10
    eval_strategy: str = "steps"
    eval_steps: int = 2000  # Reduced eval frequency for faster training
    save_strategy: str = "steps"
    save_steps: int = 500
    save_total_limit: int = 3

    # W&B
    report_to: str = "wandb"
    wandb_project: str = "cad0"

    # Other
    seed: int = 42
    dataloader_num_workers: int = 4
    group_by_length: bool = True
    packing: bool = False  # Disable packing for completion-only training


@dataclass
class DataConfig:
    """Configuration for data loading."""

    # Data paths (on Modal volume)
    train_path: str = "/data/train.jsonl"
    val_path: str = "/data/val.jsonl"
    test_path: str = "/data/test.jsonl"

    # Prompt template
    prompt_template: str = "Design: {text}\n\nVCode:\n{ir}"
    response_template: str = "\n\nVCode:\n"

    # Preprocessing
    max_samples: Optional[int] = None  # Limit samples for debugging


@dataclass
class Config:
    """Combined configuration."""

    model: ModelConfig = field(default_factory=ModelConfig)
    training: TrainingConfig = field(default_factory=TrainingConfig)
    data: DataConfig = field(default_factory=DataConfig)

    @classmethod
    def from_dict(cls, d: dict) -> "Config":
        """Create config from dictionary."""
        return cls(
            model=ModelConfig(**d.get("model", {})),
            training=TrainingConfig(**d.get("training", {})),
            data=DataConfig(**d.get("data", {})),
        )
