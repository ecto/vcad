"""
Main training script for cad0 model.

Uses PEFT/LoRA for parameter-efficient fine-tuning with QLoRA quantization.
"""

import os
from typing import Optional

import torch
from datasets import Dataset
from peft import LoraConfig, get_peft_model, prepare_model_for_kbit_training
from transformers import (
    AutoModelForCausalLM,
    AutoTokenizer,
    BitsAndBytesConfig,
    TrainingArguments,
)
from trl import SFTTrainer
from transformers import TrainerCallback

from config import Config, ModelConfig, TrainingConfig, DataConfig
from data import (
    load_dataset,
    create_formatting_function,
    create_data_collator,
    analyze_dataset,
)


class VolumeCommitCallback(TrainerCallback):
    """Callback to commit Modal volume after each checkpoint save."""

    def __init__(self, volume):
        self.volume = volume

    def on_save(self, args, state, control, **kwargs):
        """Commit volume after each checkpoint is saved."""
        if self.volume is not None:
            print(f"\n[Checkpoint {state.global_step}] Committing volume...")
            self.volume.commit()
            print(f"[Checkpoint {state.global_step}] Volume committed.")


def setup_quantization(config: ModelConfig) -> Optional[BitsAndBytesConfig]:
    """Create BitsAndBytes config for 4-bit quantization."""
    if not config.use_4bit:
        return None

    compute_dtype = getattr(torch, config.bnb_4bit_compute_dtype)

    return BitsAndBytesConfig(
        load_in_4bit=True,
        bnb_4bit_compute_dtype=compute_dtype,
        bnb_4bit_quant_type=config.bnb_4bit_quant_type,
        bnb_4bit_use_double_quant=config.use_nested_quant,
    )


def load_model_and_tokenizer(config: ModelConfig):
    """Load the base model and tokenizer with quantization."""
    # Quantization config
    bnb_config = setup_quantization(config)

    # Model kwargs
    model_kwargs = {
        "quantization_config": bnb_config,
        "device_map": "auto",
        "trust_remote_code": True,
    }

    # Add flash attention if enabled
    if config.use_flash_attention:
        model_kwargs["attn_implementation"] = "flash_attention_2"

    # Load model
    model = AutoModelForCausalLM.from_pretrained(
        config.model_name,
        torch_dtype=torch.bfloat16,
        **model_kwargs,
    )

    # Prepare for k-bit training
    if config.use_4bit:
        model = prepare_model_for_kbit_training(model)

    # Load tokenizer
    tokenizer = AutoTokenizer.from_pretrained(
        config.model_name,
        trust_remote_code=True,
    )

    # Set padding token if not set
    if tokenizer.pad_token is None:
        tokenizer.pad_token = tokenizer.eos_token
        model.config.pad_token_id = tokenizer.pad_token_id

    return model, tokenizer


def setup_lora(model, config: ModelConfig):
    """Apply LoRA configuration to the model."""
    lora_config = LoraConfig(
        r=config.lora_r,
        lora_alpha=config.lora_alpha,
        lora_dropout=config.lora_dropout,
        target_modules=config.lora_target_modules,
        bias="none",
        task_type="CAUSAL_LM",
    )

    model = get_peft_model(model, lora_config)
    model.print_trainable_parameters()

    return model


def create_training_arguments(config: TrainingConfig) -> TrainingArguments:
    """Create HuggingFace TrainingArguments."""
    return TrainingArguments(
        output_dir=config.output_dir,
        run_name=config.run_name,
        # Training
        num_train_epochs=config.num_train_epochs,
        per_device_train_batch_size=config.per_device_train_batch_size,
        per_device_eval_batch_size=config.per_device_eval_batch_size,
        gradient_accumulation_steps=config.gradient_accumulation_steps,
        learning_rate=config.learning_rate,
        weight_decay=config.weight_decay,
        warmup_ratio=config.warmup_ratio,
        lr_scheduler_type=config.lr_scheduler_type,
        max_grad_norm=config.max_grad_norm,
        # Optimization
        optim=config.optim,
        gradient_checkpointing=config.gradient_checkpointing,
        bf16=config.bf16,
        tf32=config.tf32,
        # Logging
        logging_steps=config.logging_steps,
        eval_strategy=config.eval_strategy,
        eval_steps=config.eval_steps,
        save_strategy=config.save_strategy,
        save_steps=config.save_steps,
        save_total_limit=config.save_total_limit,
        # Reporting
        report_to=config.report_to,
        # Other
        seed=config.seed,
        dataloader_num_workers=config.dataloader_num_workers,
        group_by_length=config.group_by_length,
        # Required for gradient checkpointing with LoRA
        gradient_checkpointing_kwargs={"use_reentrant": False}
        if config.gradient_checkpointing
        else None,
    )


def train_model(config: Config, volume=None) -> str:
    """
    Train the cad0 model.

    Args:
        config: Combined configuration object
        volume: Optional Modal volume to commit after each checkpoint

    Returns:
        Path to the saved model
    """
    print("=" * 60)
    print("cad0 Model Training")
    print("=" * 60)

    # Initialize W&B if configured
    if config.training.report_to == "wandb":
        import wandb

        wandb.init(
            project=config.training.wandb_project,
            name=config.training.run_name,
            config={
                "model": vars(config.model),
                "training": vars(config.training),
                "data": vars(config.data),
            },
        )

    # Load data
    print("\n1. Loading datasets...")
    train_dataset, val_dataset = load_dataset(
        config.data.train_path,
        config.data.val_path,
        config.data.max_samples,
    )
    print(f"   Train: {len(train_dataset)} examples")
    print(f"   Val: {len(val_dataset)} examples")

    # Analyze dataset
    train_stats = analyze_dataset(train_dataset)
    print(f"   Families: {len(train_stats['families'])}")
    print(f"   Text length: {train_stats['text_length_stats']}")
    print(f"   IR length: {train_stats['ir_length_stats']}")

    # Load model and tokenizer
    print("\n2. Loading model and tokenizer...")
    model, tokenizer = load_model_and_tokenizer(config.model)
    print(f"   Model: {config.model.model_name}")

    # Apply LoRA
    print("\n3. Applying LoRA...")
    model = setup_lora(model, config.model)

    # Create training arguments
    print("\n4. Setting up training...")
    training_args = create_training_arguments(config.training)

    # Create formatting function (no data collator - train on full text)
    formatting_func = create_formatting_function(config.data.prompt_template)

    # Create trainer (TRL 0.13+ uses SFTConfig for max_seq_length and packing)
    # Note: We removed the data_collator that was causing 0 loss due to
    # response key tokenization mismatch. Training on full text works fine.
    from trl import SFTConfig
    sft_config = SFTConfig(
        **training_args.to_dict(),
        max_seq_length=config.training.max_seq_length,
        packing=config.training.packing,
    )
    # Setup callbacks
    callbacks = []
    if volume is not None:
        callbacks.append(VolumeCommitCallback(volume))

    trainer = SFTTrainer(
        model=model,
        processing_class=tokenizer,
        train_dataset=train_dataset,
        eval_dataset=val_dataset,
        args=sft_config,
        formatting_func=formatting_func,
        callbacks=callbacks,
    )

    # Train
    print("\n5. Training...")
    print("-" * 60)
    trainer.train()

    # Save final model
    print("\n6. Saving model...")
    final_path = os.path.join(config.training.output_dir, "final")
    trainer.save_model(final_path)
    tokenizer.save_pretrained(final_path)
    print(f"   Saved to {final_path}")

    # Merge and save full model
    print("\n7. Merging LoRA weights...")
    merged_path = os.path.join(config.training.output_dir, "merged")
    merged_model = trainer.model.merge_and_unload()
    merged_model.save_pretrained(merged_path)
    tokenizer.save_pretrained(merged_path)
    print(f"   Saved merged model to {merged_path}")

    # Finish W&B
    if config.training.report_to == "wandb":
        wandb.finish()

    print("\n" + "=" * 60)
    print("Training complete!")
    print("=" * 60)

    return merged_path


def generate_sample(
    model,
    tokenizer,
    prompt: str,
    max_new_tokens: int = 256,
    temperature: float = 0.1,
) -> str:
    """
    Generate a sample completion for testing.

    Args:
        model: The trained model
        tokenizer: The tokenizer
        prompt: Input prompt (design description)
        max_new_tokens: Maximum tokens to generate
        temperature: Sampling temperature

    Returns:
        Generated VCode
    """
    # Format prompt
    full_prompt = f"Design: {prompt}\n\nVCode:\n"

    # Tokenize
    inputs = tokenizer(full_prompt, return_tensors="pt").to(model.device)

    # Generate
    with torch.no_grad():
        outputs = model.generate(
            **inputs,
            max_new_tokens=max_new_tokens,
            temperature=temperature,
            do_sample=temperature > 0,
            pad_token_id=tokenizer.pad_token_id,
            eos_token_id=tokenizer.eos_token_id,
        )

    # Decode
    generated = tokenizer.decode(outputs[0], skip_special_tokens=True)

    # Extract just the IR part
    if "VCode:" in generated:
        ir = generated.split("VCode:")[-1].strip()
        return ir

    return generated
