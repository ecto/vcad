#!/usr/bin/env python3
"""
Knowledge distillation from cad0 (7B) to cad0-mini (0.5B) on Lambda Labs.

Adapted from modal/distill.py for direct execution on Lambda GPU instances.
Uses DDP for multi-GPU training.

Usage:
    # Single GPU
    python distill_lambda.py

    # Multi-GPU with torchrun (recommended)
    torchrun --nproc_per_node=8 distill_lambda.py

    # Debug run
    python distill_lambda.py --max_samples 1000
"""

import argparse
import json
import os
import time
from pathlib import Path

import torch
import torch.distributed as dist
import torch.nn.functional as F
from torch.nn.parallel import DistributedDataParallel as DDP
from torch.utils.data import DataLoader
from torch.utils.data.distributed import DistributedSampler
from datasets import Dataset
from tqdm import tqdm
from transformers import (
    AutoModelForCausalLM,
    AutoTokenizer,
    get_cosine_schedule_with_warmup,
)

# Default paths on Lambda
DATA_DIR = Path("/lambda/nfs/cad0/data")
CHECKPOINT_DIR = Path("/lambda/nfs/cad0/checkpoints")


def setup_distributed():
    """Initialize distributed training if available."""
    if "RANK" in os.environ:
        rank = int(os.environ["RANK"])
        local_rank = int(os.environ["LOCAL_RANK"])
        world_size = int(os.environ["WORLD_SIZE"])
        dist.init_process_group("nccl")
        torch.cuda.set_device(local_rank)
        return rank, local_rank, world_size
    return 0, 0, 1


def cleanup_distributed():
    """Clean up distributed training."""
    if dist.is_initialized():
        dist.destroy_process_group()


def load_jsonl(path: str) -> list[dict]:
    """Load JSONL file."""
    data = []
    with open(path, "r") as f:
        for line in f:
            line = line.strip()
            if line:
                data.append(json.loads(line))
    return data


def format_prompt(text: str) -> str:
    """Format input prompt."""
    return f"Design: {text}\n\nVCode:\n"


def distill(
    teacher_model: str = "campedersen/cad0",
    student_model: str = "Qwen/Qwen2.5-0.5B",
    output_dir: str | None = None,
    num_epochs: int = 3,
    batch_size: int = 16,
    learning_rate: float = 5e-5,
    temperature: float = 2.0,
    alpha: float = 0.5,
    max_seq_length: int = 256,
    max_samples: int | None = None,
    use_wandb: bool = True,
):
    """
    Distill cad0 (7B) to cad0-mini (0.5B).

    Uses a combination of:
    1. KL divergence loss between teacher/student logits (distillation)
    2. Cross-entropy loss on ground truth labels (task loss)
    """
    # Setup distributed training
    rank, local_rank, world_size = setup_distributed()
    is_main = rank == 0

    if output_dir is None:
        output_dir = str(CHECKPOINT_DIR / "cad0-mini")

    if is_main:
        os.makedirs(output_dir, exist_ok=True)

    # Initialize W&B (only on main process)
    if use_wandb and is_main:
        import wandb

        wandb.init(
            project="cad0",
            name=f"cad0-mini-distill-lambda-{world_size}gpu",
            config={
                "teacher_model": teacher_model,
                "student_model": student_model,
                "num_epochs": num_epochs,
                "batch_size": batch_size,
                "effective_batch_size": batch_size * world_size,
                "learning_rate": learning_rate,
                "temperature": temperature,
                "alpha": alpha,
                "max_seq_length": max_seq_length,
                "world_size": world_size,
            },
        )

    run_start = time.time()

    # Load teacher model (the fine-tuned 7B model)
    # Each worker gets its own teacher on a dedicated GPU pair
    if is_main:
        print(f"Loading teacher model: {teacher_model}...")

    # For 4-worker DDP on 8 GPUs:
    # Worker 0: student on GPU 0, teacher on GPU 1
    # Worker 1: student on GPU 2, teacher on GPU 3
    # Worker 2: student on GPU 4, teacher on GPU 5
    # Worker 3: student on GPU 6, teacher on GPU 7
    if world_size > 1:
        teacher_gpu = local_rank * 2 + 1  # Odd GPUs for teacher
        student_gpu = local_rank * 2      # Even GPUs for student
        teacher_device_map = {"": f"cuda:{teacher_gpu}"}
    else:
        teacher_device_map = "auto"
        student_gpu = 0

    teacher = AutoModelForCausalLM.from_pretrained(
        teacher_model,
        torch_dtype=torch.bfloat16,
        device_map=teacher_device_map,
        trust_remote_code=True,
    )
    teacher.eval()
    for param in teacher.parameters():
        param.requires_grad = False

    # Get the actual device the teacher is on
    teacher_device = next(teacher.parameters()).device

    teacher_tokenizer = AutoTokenizer.from_pretrained(
        teacher_model,
        trust_remote_code=True,
    )

    # Load student model (base 0.5B model to be trained)
    if is_main:
        print(f"Loading student model: {student_model}...")
    student = AutoModelForCausalLM.from_pretrained(
        student_model,
        torch_dtype=torch.bfloat16,
        trust_remote_code=True,
    )

    student_tokenizer = AutoTokenizer.from_pretrained(
        student_model,
        trust_remote_code=True,
    )

    # Ensure padding token
    if teacher_tokenizer.pad_token is None:
        teacher_tokenizer.pad_token = teacher_tokenizer.eos_token
    if student_tokenizer.pad_token is None:
        student_tokenizer.pad_token = student_tokenizer.eos_token

    # Align vocab sizes BEFORE moving to GPU and DDP wrapping
    teacher_tok_vocab = len(teacher_tokenizer)
    student_tok_vocab = len(student_tokenizer)
    teacher_model_vocab = teacher.config.vocab_size
    student_model_vocab = student.config.vocab_size

    if is_main:
        print(f"Teacher tokenizer vocab: {teacher_tok_vocab}, model vocab: {teacher_model_vocab}")
        print(f"Student tokenizer vocab: {student_tok_vocab}, model vocab: {student_model_vocab}")

    target_vocab = max(teacher_tok_vocab, student_tok_vocab, teacher_model_vocab, student_model_vocab)
    if teacher_model_vocab != target_vocab:
        if is_main:
            print(f"Resizing teacher embeddings to {target_vocab}...")
        teacher.resize_token_embeddings(target_vocab)
    if student_model_vocab != target_vocab:
        if is_main:
            print(f"Resizing student embeddings to {target_vocab}...")
        student.resize_token_embeddings(target_vocab)

    # Now move student to GPU and wrap in DDP
    student.gradient_checkpointing_enable()
    student.to(f"cuda:{student_gpu}")

    if world_size > 1:
        student = DDP(student, device_ids=[student_gpu])
        student_module = student.module
    else:
        student_module = student

    tokenizer = teacher_tokenizer if teacher_tok_vocab >= student_tok_vocab else student_tokenizer

    # Load training data
    train_path = DATA_DIR / "train.jsonl"
    if is_main:
        print(f"Loading training data from {train_path}...")
    train_data = load_jsonl(str(train_path))
    if max_samples:
        train_data = train_data[:max_samples]
    if is_main:
        print(f"Loaded {len(train_data)} training examples")

    # Tokenize
    def tokenize_example(example):
        text = example["text"]
        ir = example["ir"]
        prompt = format_prompt(text)
        full_text = prompt + ir

        encoded = tokenizer(
            full_text,
            max_length=max_seq_length,
            truncation=True,
            padding="max_length",
            return_tensors="pt",
        )

        prompt_encoded = tokenizer(
            prompt,
            max_length=max_seq_length,
            truncation=True,
            return_tensors="pt",
        )
        prompt_len = prompt_encoded["input_ids"].shape[1]

        labels = encoded["input_ids"].clone()
        labels[:, :prompt_len] = -100

        return {
            "input_ids": encoded["input_ids"].squeeze(0),
            "attention_mask": encoded["attention_mask"].squeeze(0),
            "labels": labels.squeeze(0),
        }

    dataset = Dataset.from_list(train_data)
    dataset = dataset.map(tokenize_example, remove_columns=dataset.column_names, num_proc=8)
    dataset.set_format("torch")

    # Use DistributedSampler for multi-GPU
    sampler = DistributedSampler(dataset, num_replicas=world_size, rank=rank, shuffle=True) if world_size > 1 else None

    dataloader = DataLoader(
        dataset,
        batch_size=batch_size,
        shuffle=(sampler is None),
        sampler=sampler,
        num_workers=8,
        pin_memory=True,
    )

    steps_per_epoch = len(dataloader)
    total_steps = steps_per_epoch * num_epochs
    if is_main:
        print(f"Steps per epoch: {steps_per_epoch}, total: {total_steps}")

    # Optimizer and scheduler
    optimizer = torch.optim.AdamW(student.parameters(), lr=learning_rate, weight_decay=0.01)
    num_warmup_steps = total_steps // 10
    scheduler = get_cosine_schedule_with_warmup(
        optimizer, num_warmup_steps=num_warmup_steps, num_training_steps=total_steps
    )

    # Training loop
    if is_main:
        print("Starting distillation...")
    global_step = 0
    best_loss = float("inf")
    device = f"cuda:{student_gpu}"

    for epoch in range(num_epochs):
        if sampler is not None:
            sampler.set_epoch(epoch)

        student.train()
        epoch_loss = 0
        epoch_distill_loss = 0
        epoch_task_loss = 0

        progress = tqdm(dataloader, desc=f"Epoch {epoch + 1}/{num_epochs}", disable=not is_main)

        for batch in progress:
            step_start = time.time()
            input_ids = batch["input_ids"].to(device)
            attention_mask = batch["attention_mask"].to(device)
            labels = batch["labels"].to(device)

            # Get teacher logits (teacher is on dedicated GPU)
            with torch.no_grad():
                teacher_outputs = teacher(input_ids=input_ids.to(teacher_device), attention_mask=attention_mask.to(teacher_device))
                teacher_logits = teacher_outputs.logits.to(device)

            # Get student logits
            student_outputs = student(input_ids=input_ids, attention_mask=attention_mask, labels=labels)
            student_logits = student_outputs.logits
            task_loss = student_outputs.loss

            # Distillation loss
            mask = labels != -100
            if mask.any():
                teacher_probs = F.softmax(teacher_logits[mask] / temperature, dim=-1)
                student_log_probs = F.log_softmax(student_logits[mask] / temperature, dim=-1)
                distill_loss = F.kl_div(student_log_probs, teacher_probs, reduction="batchmean") * (temperature**2)
            else:
                distill_loss = torch.tensor(0.0, device=device)

            # Combined loss
            loss = alpha * distill_loss + (1 - alpha) * task_loss

            optimizer.zero_grad()
            loss.backward()
            torch.nn.utils.clip_grad_norm_(student.parameters(), 1.0)
            optimizer.step()
            scheduler.step()

            epoch_loss += loss.item()
            epoch_distill_loss += distill_loss.item()
            epoch_task_loss += task_loss.item()
            global_step += 1
            step_time = time.time() - step_start
            tokens_per_sec = input_ids.numel() * world_size / step_time if step_time > 0 else 0

            if is_main:
                progress.set_postfix(
                    {"loss": f"{loss.item():.4f}", "distill": f"{distill_loss.item():.4f}", "task": f"{task_loss.item():.4f}"}
                )

            if use_wandb and is_main and global_step % 50 == 0:
                import wandb

                wandb.log(
                    {
                        "loss": loss.item(),
                        "distill_loss": distill_loss.item(),
                        "task_loss": task_loss.item(),
                        "learning_rate": scheduler.get_last_lr()[0],
                        "epoch": epoch,
                        "step": global_step,
                        "step_time_s": step_time,
                        "tokens_per_sec": tokens_per_sec,
                    }
                )

            if is_main and global_step % 500 == 0:
                elapsed_min = (time.time() - run_start) / 60
                print(
                    f"Step {global_step}/{total_steps} | "
                    f"loss={loss.item():.4f} distill={distill_loss.item():.4f} task={task_loss.item():.4f} | "
                    f"{tokens_per_sec:.0f} tok/s | {elapsed_min:.1f}min"
                )

        # Epoch summary
        avg_loss = epoch_loss / len(dataloader)
        avg_distill = epoch_distill_loss / len(dataloader)
        avg_task = epoch_task_loss / len(dataloader)

        if is_main:
            print(f"Epoch {epoch + 1}: loss={avg_loss:.4f}, distill={avg_distill:.4f}, task={avg_task:.4f}")

            if use_wandb:
                import wandb
                wandb.log({"epoch_loss": avg_loss, "epoch_distill_loss": avg_distill, "epoch_task_loss": avg_task})

            # Save checkpoint
            if avg_loss < best_loss:
                best_loss = avg_loss
                checkpoint_path = os.path.join(output_dir, f"epoch-{epoch + 1}")
                print(f"Saving checkpoint to {checkpoint_path}...")
                student_module.save_pretrained(checkpoint_path)
                tokenizer.save_pretrained(checkpoint_path)

        # Sync before next epoch
        if world_size > 1:
            dist.barrier()

    # Save final model (only main process)
    if is_main:
        print(f"Saving final model to {output_dir}...")
        student_module.save_pretrained(output_dir)
        tokenizer.save_pretrained(output_dir)

        if use_wandb:
            import wandb
            wandb.finish()

        total_time = (time.time() - run_start) / 60
        print(f"Distillation complete in {total_time:.1f} minutes!")

    cleanup_distributed()
    return output_dir


def main():
    parser = argparse.ArgumentParser(description="Distill cad0 to cad0-mini")
    parser.add_argument("--teacher_model", default="campedersen/cad0", help="Teacher model")
    parser.add_argument("--student_model", default="Qwen/Qwen2.5-0.5B", help="Student model")
    parser.add_argument("--output_dir", default=None, help="Output directory")
    parser.add_argument("--num_epochs", type=int, default=3, help="Number of epochs")
    parser.add_argument("--batch_size", type=int, default=16, help="Batch size per GPU")
    parser.add_argument("--learning_rate", type=float, default=5e-5, help="Learning rate")
    parser.add_argument("--temperature", type=float, default=2.0, help="Distillation temperature")
    parser.add_argument("--alpha", type=float, default=0.5, help="Weight for distillation loss")
    parser.add_argument("--max_seq_length", type=int, default=256, help="Max sequence length")
    parser.add_argument("--max_samples", type=int, default=None, help="Limit samples (debug)")
    parser.add_argument("--no_wandb", action="store_true", help="Disable W&B logging")
    args = parser.parse_args()

    distill(
        teacher_model=args.teacher_model,
        student_model=args.student_model,
        output_dir=args.output_dir,
        num_epochs=args.num_epochs,
        batch_size=args.batch_size,
        learning_rate=args.learning_rate,
        temperature=args.temperature,
        alpha=args.alpha,
        max_seq_length=args.max_seq_length,
        max_samples=args.max_samples,
        use_wandb=not args.no_wandb,
    )


if __name__ == "__main__":
    main()
