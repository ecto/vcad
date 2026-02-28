"""
Modal app definition for cad0 training.

Usage:
    # Setup secrets
    modal secret create huggingface-secret HUGGING_FACE_HUB_TOKEN=<token>
    modal secret create wandb-secret WANDB_API_KEY=<key>

    # Create volume and upload data
    modal volume create vcad-training-vol
    modal volume put vcad-training-vol packages/training/data/train.jsonl /data/train.jsonl
    modal volume put vcad-training-vol packages/training/data/val.jsonl /data/val.jsonl

    # Run training
    modal run modal_app.py::train

    # Download model
    modal volume get vcad-training-vol /data/checkpoints/merged ./cad0-model
"""

import modal

# Modal app
app = modal.App("cad0-training")

# Persistent volume for data and checkpoints
volume = modal.Volume.from_name("vcad-training-vol", create_if_missing=True)

# Container image with all dependencies
# Use CUDA image for flash-attn compilation
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
        "fastapi[standard]",
    )
    .pip_install(
        "flash-attn",
        extra_options="--no-build-isolation",
    )
    .env({"HF_HUB_ENABLE_HF_TRANSFER": "1"})
    .add_local_dir(".", "/root", copy=True)
)


@app.function(
    image=image,
    gpu="H100",  # 1x H100 is optimal: 2.3s/step 🔥
    volumes={"/data": volume},
        secrets=[
        modal.Secret.from_name("huggingface-secret"),
        modal.Secret.from_name("wandb-secret"),
    ],
    timeout=60 * 60 * 12,  # 12 hours
)
def train(
    model_name: str = "Qwen/Qwen2.5-Coder-7B",
    lora_r: int = 64,
    lora_alpha: int = 128,
    num_epochs: int = 1,
    batch_size: int = 16,
    grad_accum: int = 4,
    learning_rate: float = 2e-4,
    max_seq_length: int = 1024,
    max_samples: int | None = None,
    run_name: str | None = None,
):
    """
    Train the cad0 model with LoRA fine-tuning.

    Args:
        model_name: HuggingFace model ID
        lora_r: LoRA rank
        lora_alpha: LoRA alpha
        num_epochs: Number of training epochs
        batch_size: Per-device batch size
        grad_accum: Gradient accumulation steps
        learning_rate: Learning rate
        max_seq_length: Maximum sequence length
        max_samples: Limit samples for debugging (None = use all)
        run_name: W&B run name
    """
    from config import Config, ModelConfig, TrainingConfig, DataConfig
    from train import train_model

    # Force reload volume to get latest data
    volume.reload()

    # Build config
    config = Config(
        model=ModelConfig(
            model_name=model_name,
            lora_r=lora_r,
            lora_alpha=lora_alpha,
        ),
        training=TrainingConfig(
            output_dir="/data/checkpoints",
            run_name=run_name or f"cad0-{model_name.split('/')[-1]}-lora",
            num_train_epochs=num_epochs,
            per_device_train_batch_size=batch_size,
            gradient_accumulation_steps=grad_accum,
            learning_rate=learning_rate,
            max_seq_length=max_seq_length,
        ),
        data=DataConfig(
            train_path="/data/train.jsonl",
            val_path="/data/val.jsonl",
            max_samples=max_samples,
        ),
    )

    # Run training (pass volume for checkpoint commits)
    output_path = train_model(config, volume=volume)

    # Commit volume changes
    volume.commit()

    return output_path


@app.function(
    image=image,
    gpu="A100-80GB",
    volumes={"/data": volume},
        secrets=[modal.Secret.from_name("huggingface-secret")],
    timeout=60 * 60,  # 1 hour
)
def evaluate(
    model_path: str = "/data/checkpoints/merged",
    test_path: str = "/data/test.jsonl",
    max_samples: int | None = 500,
):
    """
    Evaluate a trained model on the test set.

    Args:
        model_path: Path to the merged model
        test_path: Path to test JSONL file
        max_samples: Maximum samples to evaluate
    """
    import torch
    from transformers import AutoModelForCausalLM, AutoTokenizer
    from datasets import Dataset

    from data import load_jsonl
    from eval import evaluate_model, print_eval_report

    # Load model
    print(f"Loading model from {model_path}...")
    model = AutoModelForCausalLM.from_pretrained(
        model_path,
        torch_dtype=torch.bfloat16,
        device_map="auto",
        trust_remote_code=True,
    )
    tokenizer = AutoTokenizer.from_pretrained(
        model_path,
        trust_remote_code=True,
    )

    # Load test data
    print(f"Loading test data from {test_path}...")
    test_data = load_jsonl(test_path)
    test_dataset = Dataset.from_list(test_data)

    # Evaluate
    print("Running evaluation...")
    metrics = evaluate_model(
        model,
        tokenizer,
        test_dataset,
        validate_geometry=False,
        max_samples=max_samples,
    )

    print_eval_report(metrics)

    return {
        "syntax_accuracy": metrics.syntax_accuracy,
        "exact_match_accuracy": metrics.exact_match_accuracy,
        "total": metrics.total,
    }


@app.function(
    image=image,
    gpu="A100-80GB",
    volumes={"/data": volume},
        secrets=[modal.Secret.from_name("huggingface-secret")],
    timeout=60 * 10,  # 10 minutes
)
def generate(
    prompts: list[str],
    model_path: str = "/data/checkpoints/merged",
    temperature: float = 0.1,
    max_new_tokens: int = 256,
):
    """
    Generate VCode from text prompts.

    Args:
        prompts: List of design descriptions
        model_path: Path to the model
        temperature: Sampling temperature
        max_new_tokens: Maximum tokens to generate
    """
    import torch
    from transformers import AutoModelForCausalLM, AutoTokenizer

    from train import generate_sample

    # Load model
    print(f"Loading model from {model_path}...")
    model = AutoModelForCausalLM.from_pretrained(
        model_path,
        torch_dtype=torch.bfloat16,
        device_map="auto",
        trust_remote_code=True,
    )
    tokenizer = AutoTokenizer.from_pretrained(
        model_path,
        trust_remote_code=True,
    )

    # Generate
    results = []
    for prompt in prompts:
        print(f"\nPrompt: {prompt}")
        ir = generate_sample(
            model,
            tokenizer,
            prompt,
            max_new_tokens=max_new_tokens,
            temperature=temperature,
        )
        print(f"IR:\n{ir}")
        results.append({"prompt": prompt, "ir": ir})

    return results


@app.function(
    image=image,
    volumes={"/data": volume},
    timeout=60 * 5,
)
def upload_data(
    train_data: bytes,
    val_data: bytes,
    test_data: bytes | None = None,
):
    """
    Upload training data to the Modal volume.

    Args:
        train_data: Training JSONL file contents
        val_data: Validation JSONL file contents
        test_data: Optional test JSONL file contents
    """
    import os

    # Ensure directory exists
    os.makedirs("/data", exist_ok=True)

    # Write files
    with open("/data/train.jsonl", "wb") as f:
        f.write(train_data)
    print(f"Wrote {len(train_data)} bytes to /data/train.jsonl")

    with open("/data/val.jsonl", "wb") as f:
        f.write(val_data)
    print(f"Wrote {len(val_data)} bytes to /data/val.jsonl")

    if test_data:
        with open("/data/test.jsonl", "wb") as f:
            f.write(test_data)
        print(f"Wrote {len(test_data)} bytes to /data/test.jsonl")

    # Commit volume
    volume.commit()

    return "Data uploaded successfully"


@app.function(
    image=image,
    volumes={"/data": volume},
    timeout=60,
)
def debug_volume():
    """Debug function to check volume contents."""
    import os
    import subprocess

    # Force reload
    volume.reload()

    # List files
    print("Files in /data:")
    result = subprocess.run(["ls", "-la", "/data"], capture_output=True, text=True)
    print(result.stdout)

    # Check train.jsonl line count
    result = subprocess.run(["wc", "-l", "/data/train.jsonl"], capture_output=True, text=True)
    print(f"train.jsonl lines: {result.stdout.strip()}")

    # First line
    with open("/data/train.jsonl", "r") as f:
        first_line = f.readline()
        print(f"First line length: {len(first_line)}")
        print(f"First 100 chars: {first_line[:100]}")


# ============================================================
# Inference endpoint for API
# ============================================================

@app.cls(
    image=image,
    gpu="A10G",  # Cheaper than A100 for inference
    volumes={"/data": volume},
    secrets=[modal.Secret.from_name("huggingface-secret")],
    container_idle_timeout=300,  # Keep warm for 5 min
)
class Inference:
    """Inference endpoint with model caching."""

    @modal.enter()
    def load_model(self):
        """Load model once when container starts."""
        import torch
        from transformers import AutoModelForCausalLM, AutoTokenizer

        model_path = "/data/checkpoints/merged"
        print(f"Loading model from {model_path}...")

        self.model = AutoModelForCausalLM.from_pretrained(
            model_path,
            torch_dtype=torch.bfloat16,
            device_map="auto",
            trust_remote_code=True,
        )
        self.tokenizer = AutoTokenizer.from_pretrained(
            model_path,
            trust_remote_code=True,
        )
        print("Model loaded!")

    @modal.web_endpoint(method="POST")
    def infer(self, request: dict):
        """
        Generate VCode from a text prompt.

        Request body:
            {"prompt": "design description", "temperature": 0.1, "max_tokens": 256}

        Returns:
            {"ir": "generated VCode", "tokens": 42}
        """
        import torch

        prompt = request.get("prompt", "")
        temperature = request.get("temperature", 0.1)
        max_tokens = request.get("max_tokens", 256)

        if not prompt:
            return {"error": "prompt required"}, 400

        # Format prompt
        system = "You are a CAD assistant. Generate VCode code for the given design."
        formatted = f"Design: {prompt}\n\nVCode:\n"

        messages = [
            {"role": "system", "content": system},
            {"role": "user", "content": formatted},
        ]

        # Tokenize
        text = self.tokenizer.apply_chat_template(
            messages,
            tokenize=False,
            add_generation_prompt=True,
        )
        inputs = self.tokenizer(text, return_tensors="pt").to(self.model.device)

        # Generate with stop sequences
        stop_strings = ["\n\n", "User", "Now:", "Assistant"]
        with torch.no_grad():
            outputs = self.model.generate(
                **inputs,
                max_new_tokens=max_tokens,
                temperature=temperature if temperature > 0 else None,
                do_sample=temperature > 0,
                pad_token_id=self.tokenizer.eos_token_id,
                stop_strings=stop_strings,
                tokenizer=self.tokenizer,
            )

        # Decode
        generated = outputs[0][inputs["input_ids"].shape[1]:]
        ir = self.tokenizer.decode(generated, skip_special_tokens=True)

        # Clean up - truncate at stop patterns
        ir = ir.strip()
        for stop in stop_strings:
            if stop in ir:
                ir = ir.split(stop)[0].strip()
        if ir.startswith("```"):
            ir = ir.split("```")[1].strip()
            if ir.startswith("ir") or ir.startswith("text"):
                ir = ir.split("\n", 1)[1] if "\n" in ir else ir

        return {"ir": ir, "tokens": len(generated)}


@app.local_entrypoint()
def main(
    action: str = "train",
    max_samples: int | None = None,
    run_name: str | None = None,
):
    """
    Local entrypoint for cad0 training.

    Args:
        action: Action to perform (train, evaluate, generate)
        max_samples: Limit samples for debugging
        run_name: W&B run name
    """
    if action == "train":
        # Use spawn() for detached execution - training continues even if client disconnects
        fc = train.spawn(
            max_samples=max_samples,
            run_name=run_name,
        )
        print(f"Training started! Function call ID: {fc.object_id}")
        print("Training will continue in the cloud even if you disconnect.")
        print("Check progress at: https://modal.com/apps/ecto/main/deployed/cad0-training")

    elif action == "evaluate":
        result = evaluate.remote(max_samples=max_samples)
        print(f"Evaluation results: {result}")

    elif action == "generate":
        prompts = [
            "50x30mm mounting plate with 4 corner holes",
            "10mm diameter 25mm tall standoff",
            "L-bracket with mounting holes",
        ]
        results = generate.remote(prompts)
        for r in results:
            print(f"\n{r['prompt']}:\n{r['ir']}")

    else:
        print(f"Unknown action: {action}")
        print("Available actions: train, evaluate, generate")
