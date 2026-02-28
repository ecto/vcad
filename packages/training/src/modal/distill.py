"""
Knowledge distillation from cad0 (7B) to cad0-mini (0.5B).

Distills the fine-tuned Qwen2.5-Coder-7B model to Qwen2.5-0.5B for
browser inference via Transformers.js.

Usage:
    # Run distillation on Modal
    modal run distill.py

    # Export to ONNX after distillation
    modal run distill.py --action export-onnx
"""

import modal

# Reuse the same app and infrastructure from modal_app.py
app = modal.App("cad0-distill")

volume = modal.Volume.from_name("vcad-training-vol", create_if_missing=True)

image = (
    modal.Image.from_registry(
        "nvidia/cuda:12.4.0-devel-ubuntu22.04",
        add_python="3.11",
    )
    .apt_install("git")
    .pip_install(
        "packaging",
        "ninja",
        "wheel",
    )
    .pip_install(
        "torch==2.5.1",
        extra_options="--extra-index-url https://download.pytorch.org/whl/cu124",
    )
    .pip_install(
        "transformers==4.47.0",
        "peft==0.14.0",
        "trl==0.13.0",
        "datasets==3.2.0",
        "bitsandbytes==0.45.0",
        "accelerate==1.2.1",
        "wandb==0.19.1",
        "hf_transfer",
        "optimum[exporters,onnxruntime]",  # For ONNX export
    )
    .pip_install(
        "flash-attn",
        extra_options="--no-build-isolation",
    )
    .env(
        {
            "HF_HUB_ENABLE_HF_TRANSFER": "1",
            "PYTORCH_CUDA_ALLOC_CONF": "expandable_segments:True",
        }
    )
)


@app.function(
    image=image,
    gpu="A100-80GB",  # Need space for both teacher and student
    volumes={"/data": volume},
    secrets=[
        modal.Secret.from_name("huggingface-secret"),
        modal.Secret.from_name("wandb-secret"),
    ],
    timeout=60 * 60 * 10,  # 10 hours
)
def distill(
    teacher_path: str = "/data/checkpoints/merged",
    student_model: str = "Qwen/Qwen2.5-0.5B",
    output_dir: str = "/data/checkpoints/cad0-mini",
    num_epochs: int = 3,
    batch_size: int = 16,
    learning_rate: float = 5e-5,
    temperature: float = 2.0,
    alpha: float = 0.5,  # Weight for distillation loss vs task loss
    max_seq_length: int = 256,
    max_samples: int | None = None,
):
    """
    Distill cad0 (7B) to cad0-mini (0.5B).

    Uses a combination of:
    1. KL divergence loss between teacher/student logits (distillation)
    2. Cross-entropy loss on ground truth labels (task loss)

    Args:
        teacher_path: Path to trained 7B model
        student_model: HuggingFace model ID for student
        output_dir: Where to save distilled model
        num_epochs: Training epochs
        batch_size: Per-device batch size
        learning_rate: Learning rate for student
        temperature: Softmax temperature for distillation
        alpha: Weight for distillation loss (1-alpha for task loss)
        max_seq_length: Maximum sequence length
        max_samples: Limit samples for debugging
    """
    import json
    import torch
    import torch.nn.functional as F
    from torch.utils.data import DataLoader
    from transformers import (
        AutoModelForCausalLM,
        AutoTokenizer,
        get_cosine_schedule_with_warmup,
    )
    from datasets import Dataset
    from tqdm import tqdm
    import wandb
    import time

    # Force reload volume
    volume.reload()

    # Initialize W&B
    wandb.init(
        project="cad0",
        name="cad0-mini-distill",
        config={
            "teacher_path": teacher_path,
            "student_model": student_model,
            "num_epochs": num_epochs,
            "batch_size": batch_size,
            "learning_rate": learning_rate,
            "temperature": temperature,
            "alpha": alpha,
            "max_seq_length": max_seq_length,
        },
    )

    run_start = time.time()

    print(f"Loading teacher model from {teacher_path}...")
    teacher = AutoModelForCausalLM.from_pretrained(
        teacher_path,
        torch_dtype=torch.bfloat16,
        device_map="auto",
        trust_remote_code=True,
    )
    teacher.eval()
    for param in teacher.parameters():
        param.requires_grad = False

    teacher_tokenizer = AutoTokenizer.from_pretrained(
        teacher_path,
        trust_remote_code=True,
    )

    print(f"Loading student model: {student_model}...")
    student = AutoModelForCausalLM.from_pretrained(
        student_model,
        torch_dtype=torch.bfloat16,
        trust_remote_code=True,
    )
    student.gradient_checkpointing_enable()
    student.to("cuda")

    student_tokenizer = AutoTokenizer.from_pretrained(
        student_model,
        trust_remote_code=True,
    )

    # Ensure padding token
    if teacher_tokenizer.pad_token is None:
        teacher_tokenizer.pad_token = teacher_tokenizer.eos_token
    if student_tokenizer.pad_token is None:
        student_tokenizer.pad_token = student_tokenizer.eos_token

    teacher_tok_vocab = len(teacher_tokenizer)
    student_tok_vocab = len(student_tokenizer)
    teacher_model_vocab = teacher.config.vocab_size
    student_model_vocab = student.config.vocab_size

    print(f"Teacher tokenizer vocab size: {teacher_tok_vocab}")
    print(f"Student tokenizer vocab size: {student_tok_vocab}")
    print(f"Teacher model vocab size: {teacher_model_vocab}")
    print(f"Student model vocab size: {student_model_vocab}")

    target_vocab = max(
        teacher_tok_vocab,
        student_tok_vocab,
        teacher_model_vocab,
        student_model_vocab,
    )
    if teacher_model_vocab != target_vocab:
        print(f"Resizing teacher embeddings to {target_vocab}...")
        teacher.resize_token_embeddings(target_vocab)
    if student_model_vocab != target_vocab:
        print(f"Resizing student embeddings to {target_vocab}...")
        student.resize_token_embeddings(target_vocab)

    tokenizer = (
        teacher_tokenizer
        if teacher_tok_vocab >= student_tok_vocab
        else student_tokenizer
    )

    # Load training data
    print("Loading training data...")
    def load_jsonl(path: str) -> list[dict]:
        data = []
        with open(path, "r") as f:
            for line in f:
                line = line.strip()
                if line:
                    data.append(json.loads(line))
        return data

    def format_prompt(text: str) -> str:
        return f"Design: {text}\n\nVCode:\n"

    train_data = load_jsonl("/data/train.jsonl")
    if max_samples:
        train_data = train_data[:max_samples]

    print(f"Loaded {len(train_data)} training examples")

    # Prepare dataset
    def tokenize_example(example):
        # Format as chat
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

        # Find where the response starts
        prompt_encoded = tokenizer(
            prompt,
            max_length=max_seq_length,
            truncation=True,
            return_tensors="pt",
        )
        prompt_len = prompt_encoded["input_ids"].shape[1]

        # Create labels (mask prompt tokens)
        labels = encoded["input_ids"].clone()
        labels[:, :prompt_len] = -100  # Ignore prompt in loss

        return {
            "input_ids": encoded["input_ids"].squeeze(0),
            "attention_mask": encoded["attention_mask"].squeeze(0),
            "labels": labels.squeeze(0),
        }

    dataset = Dataset.from_list(train_data)
    dataset = dataset.map(
        tokenize_example,
        remove_columns=dataset.column_names,
        num_proc=4,
    )
    dataset.set_format("torch")

    dataloader = DataLoader(
        dataset,
        batch_size=batch_size,
        shuffle=True,
        num_workers=4,
        pin_memory=True,
    )

    steps_per_epoch = len(dataloader)
    total_steps = steps_per_epoch * num_epochs
    print(f"Steps per epoch: {steps_per_epoch} (total: {total_steps})")

    # Optimizer and scheduler
    optimizer = torch.optim.AdamW(
        student.parameters(),
        lr=learning_rate,
        weight_decay=0.01,
    )

    num_training_steps = len(dataloader) * num_epochs
    num_warmup_steps = num_training_steps // 10

    scheduler = get_cosine_schedule_with_warmup(
        optimizer,
        num_warmup_steps=num_warmup_steps,
        num_training_steps=num_training_steps,
    )

    # Training loop
    print("Starting distillation...")
    global_step = 0

    for epoch in range(num_epochs):
        student.train()
        epoch_loss = 0
        epoch_distill_loss = 0
        epoch_task_loss = 0

        progress = tqdm(dataloader, desc=f"Epoch {epoch + 1}/{num_epochs}")

        for batch in progress:
            step_start = time.time()
            input_ids = batch["input_ids"].to("cuda")
            attention_mask = batch["attention_mask"].to("cuda")
            labels = batch["labels"].to("cuda")

            # Get teacher logits (no grad)
            with torch.no_grad():
                teacher_outputs = teacher(
                    input_ids=input_ids,
                    attention_mask=attention_mask,
                )
                teacher_logits = teacher_outputs.logits

            # Get student logits
            student_outputs = student(
                input_ids=input_ids,
                attention_mask=attention_mask,
                labels=labels,
            )
            student_logits = student_outputs.logits
            task_loss = student_outputs.loss

            # Distillation loss (KL divergence with temperature)
            # Only compute on non-masked positions
            mask = labels != -100
            if mask.any():
                teacher_probs = F.softmax(teacher_logits[mask] / temperature, dim=-1)
                student_log_probs = F.log_softmax(student_logits[mask] / temperature, dim=-1)
                distill_loss = F.kl_div(
                    student_log_probs,
                    teacher_probs,
                    reduction="batchmean",
                ) * (temperature ** 2)
            else:
                distill_loss = torch.tensor(0.0, device="cuda")

            # Combined loss
            loss = alpha * distill_loss + (1 - alpha) * task_loss

            # Backward
            optimizer.zero_grad()
            loss.backward()
            torch.nn.utils.clip_grad_norm_(student.parameters(), 1.0)
            optimizer.step()
            scheduler.step()

            # Logging
            epoch_loss += loss.item()
            epoch_distill_loss += distill_loss.item()
            epoch_task_loss += task_loss.item()
            global_step += 1
            step_time = time.time() - step_start
            tokens = input_ids.numel()
            tokens_per_sec = tokens / step_time if step_time > 0 else 0.0

            progress.set_postfix({
                "loss": f"{loss.item():.4f}",
                "distill": f"{distill_loss.item():.4f}",
                "task": f"{task_loss.item():.4f}",
            })

            if global_step % 50 == 0:
                mem_alloc_gb = torch.cuda.memory_allocated() / (1024 ** 3)
                mem_reserved_gb = torch.cuda.memory_reserved() / (1024 ** 3)
                mem_max_alloc_gb = torch.cuda.max_memory_allocated() / (1024 ** 3)
                wandb.log({
                    "loss": loss.item(),
                    "distill_loss": distill_loss.item(),
                    "task_loss": task_loss.item(),
                    "learning_rate": scheduler.get_last_lr()[0],
                    "epoch": epoch,
                    "step": global_step,
                    "step_time_s": step_time,
                    "tokens_per_sec": tokens_per_sec,
                    "gpu_mem_alloc_gb": mem_alloc_gb,
                    "gpu_mem_reserved_gb": mem_reserved_gb,
                    "gpu_mem_max_alloc_gb": mem_max_alloc_gb,
                })

            if global_step % 200 == 0:
                elapsed_min = (time.time() - run_start) / 60.0
                print(
                    "Heartbeat: "
                    f"step={global_step}/{total_steps} "
                    f"loss={loss.item():.4f} "
                    f"distill={distill_loss.item():.4f} "
                    f"task={task_loss.item():.4f} "
                    f"step_time_s={step_time:.2f} "
                    f"tokens_per_sec={tokens_per_sec:.0f} "
                    f"elapsed_min={elapsed_min:.1f}"
                )

        # Epoch summary
        avg_loss = epoch_loss / len(dataloader)
        avg_distill = epoch_distill_loss / len(dataloader)
        avg_task = epoch_task_loss / len(dataloader)

        print(f"Epoch {epoch + 1}: loss={avg_loss:.4f}, distill={avg_distill:.4f}, task={avg_task:.4f}")

        wandb.log({
            "epoch_loss": avg_loss,
            "epoch_distill_loss": avg_distill,
            "epoch_task_loss": avg_task,
            "epoch": epoch + 1,
        })

    # Save model
    print(f"Saving distilled model to {output_dir}...")
    os.makedirs(output_dir, exist_ok=True)
    student.save_pretrained(output_dir)
    tokenizer.save_pretrained(output_dir)

    # Commit volume
    volume.commit()

    wandb.finish()

    print("Distillation complete!")
    return output_dir


@app.function(
    image=image,
    gpu="A10G",  # Smaller GPU sufficient for export
    volumes={"/data": volume},
    secrets=[modal.Secret.from_name("huggingface-secret")],
    timeout=60 * 60,  # 1 hour
)
def export_onnx(
    model_path: str = "/data/checkpoints/cad0-mini",
    output_dir: str = "/data/checkpoints/cad0-mini-onnx",
    quantize: bool = True,
):
    """
    Export distilled model to ONNX format for Transformers.js.

    Args:
        model_path: Path to distilled PyTorch model
        output_dir: Where to save ONNX model
        quantize: Whether to quantize to int8/int4
    """
    import os
    from optimum.exporters.onnx import main_export
    from optimum.onnxruntime import ORTModelForCausalLM
    from transformers import AutoTokenizer

    volume.reload()

    print(f"Loading model from {model_path}...")

    # Export to ONNX
    print(f"Exporting to ONNX at {output_dir}...")
    os.makedirs(output_dir, exist_ok=True)

    main_export(
        model_name_or_path=model_path,
        output=output_dir,
        task="text-generation-with-past",
        opset=17,
        device="cuda",
        fp16=True,
    )

    if quantize:
        print("Quantizing model...")
        from optimum.onnxruntime import ORTQuantizer
        from optimum.onnxruntime.configuration import AutoQuantizationConfig

        # Load ONNX model
        ort_model = ORTModelForCausalLM.from_pretrained(
            output_dir,
            export=False,
        )

        # Quantize to int4 for smallest size
        quantizer = ORTQuantizer.from_pretrained(ort_model)
        qconfig = AutoQuantizationConfig.avx512_vnni(
            is_static=False,
            per_channel=True,
        )

        quantized_dir = output_dir + "-q4"
        quantizer.quantize(
            save_dir=quantized_dir,
            quantization_config=qconfig,
        )

        print(f"Quantized model saved to {quantized_dir}")
        output_dir = quantized_dir

    # Copy tokenizer
    tokenizer = AutoTokenizer.from_pretrained(model_path)
    tokenizer.save_pretrained(output_dir)

    volume.commit()

    print(f"ONNX export complete: {output_dir}")
    return output_dir


@app.function(
    image=image,
    volumes={"/data": volume},
    secrets=[modal.Secret.from_name("huggingface-secret")],
    timeout=60 * 30,
)
def upload_to_hf(
    model_path: str = "/data/checkpoints/cad0-mini-onnx-q4",
    repo_id: str = "campedersen/cad0-mini",
    private: bool = False,
):
    """
    Upload model to HuggingFace Hub.

    Args:
        model_path: Path to model directory
        repo_id: HuggingFace repo ID (e.g., "campedersen/cad0-mini")
        private: Whether to make repo private
    """
    from huggingface_hub import HfApi

    volume.reload()

    api = HfApi()

    print(f"Uploading {model_path} to {repo_id}...")

    api.upload_folder(
        folder_path=model_path,
        repo_id=repo_id,
        repo_type="model",
        create_pr=False,
    )

    print(f"Upload complete: https://huggingface.co/{repo_id}")
    return repo_id


@app.local_entrypoint()
def main(
    action: str = "distill",
    max_samples: int | None = None,
):
    """
    Local entrypoint for distillation pipeline.

    Args:
        action: Action to perform (distill, export-onnx, upload)
        max_samples: Limit samples for debugging
    """
    if action == "distill":
        fc = distill.spawn(max_samples=max_samples)
        print(f"Distillation started! Function call ID: {fc.object_id}")
        print("Check progress at W&B: https://wandb.ai/ecto/cad0")

    elif action == "export-onnx":
        result = export_onnx.remote()
        print(f"ONNX export complete: {result}")

    elif action == "upload":
        # Upload both models
        upload_to_hf.remote(
            model_path="/data/checkpoints/merged",
            repo_id="campedersen/cad0",
        )
        upload_to_hf.remote(
            model_path="/data/checkpoints/cad0-mini-onnx-q4",
            repo_id="campedersen/cad0-mini",
        )
        print("Upload complete!")

    elif action == "all":
        # Run full pipeline
        print("Running full distillation pipeline...")
        distill_path = distill.remote(max_samples=max_samples)
        print(f"Distillation complete: {distill_path}")

        onnx_path = export_onnx.remote()
        print(f"ONNX export complete: {onnx_path}")

        upload_to_hf.remote(
            model_path="/data/checkpoints/cad0-mini-onnx-q4",
            repo_id="campedersen/cad0-mini",
        )
        print("Pipeline complete!")

    else:
        print(f"Unknown action: {action}")
        print("Available actions: distill, export-onnx, upload, all")
