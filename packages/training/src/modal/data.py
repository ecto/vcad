"""
Data loading and preprocessing for cad0 training.
"""

import json
from pathlib import Path
from typing import Optional

from datasets import Dataset
from transformers import PreTrainedTokenizer


def load_jsonl(path: str) -> list[dict]:
    """Load a JSONL file into a list of dictionaries."""
    data = []
    with open(path, "r") as f:
        for line in f:
            line = line.strip()
            if line:
                data.append(json.loads(line))
    return data


def load_dataset(
    train_path: str,
    val_path: str,
    max_samples: Optional[int] = None,
) -> tuple[Dataset, Dataset]:
    """
    Load training and validation datasets from JSONL files.

    Args:
        train_path: Path to training JSONL file
        val_path: Path to validation JSONL file
        max_samples: Optional limit on number of samples (for debugging)

    Returns:
        Tuple of (train_dataset, val_dataset)
    """
    train_data = load_jsonl(train_path)
    val_data = load_jsonl(val_path)

    if max_samples is not None:
        train_data = train_data[:max_samples]
        val_data = val_data[: max_samples // 10]

    train_dataset = Dataset.from_list(train_data)
    val_dataset = Dataset.from_list(val_data)

    return train_dataset, val_dataset


def format_prompt(text: str) -> str:
    """
    Format a design description into a prompt for inference.

    This is the same prompt format used during training, but without the IR.
    Used for inference and distillation.

    Args:
        text: Design description

    Returns:
        Formatted prompt string
    """
    return f"Design: {text}\n\nVCode:\n"


def format_for_training(
    example: dict,
    prompt_template: str = "Design: {text}\n\nVCode:\n{ir}",
) -> dict:
    """
    Format a single example for training.

    The format is completion-style where the model learns to generate
    the VCode given a design description.

    Args:
        example: Dictionary with 'text' and 'ir' keys
        prompt_template: Template string with {text} and {ir} placeholders

    Returns:
        Dictionary with 'text' key containing formatted training example
    """
    formatted = prompt_template.format(text=example["text"], ir=example["ir"])
    return {"text": formatted}


def create_formatting_function(prompt_template: str):
    """Create a formatting function for use with SFTTrainer."""

    def formatting_func(examples: dict) -> list[str]:
        """Format batch of examples for SFTTrainer."""
        texts = []
        for text, ir in zip(examples["text"], examples["ir"]):
            formatted = prompt_template.format(text=text, ir=ir)
            texts.append(formatted)
        return texts

    return formatting_func


def create_data_collator(
    tokenizer: PreTrainedTokenizer,
    response_template: str = "\n\nVCode:\n",
):
    """
    Create a data collator that only computes loss on the response tokens.

    This uses TRL's DataCollatorForCompletionOnlyLM to mask prompt tokens,
    so the model only learns to generate the VCode output.

    Args:
        tokenizer: The tokenizer to use
        response_template: String that marks the start of the response

    Returns:
        DataCollatorForCompletionOnlyLM instance
    """
    from trl import DataCollatorForCompletionOnlyLM

    # The response template marks where the completion begins
    # Everything before this is masked (loss = 0)
    collator = DataCollatorForCompletionOnlyLM(
        response_template=response_template,
        tokenizer=tokenizer,
    )

    return collator


def analyze_dataset(dataset: Dataset) -> dict:
    """
    Compute statistics about a dataset.

    Args:
        dataset: HuggingFace Dataset to analyze

    Returns:
        Dictionary with statistics
    """
    stats = {
        "total_examples": len(dataset),
        "families": {},
        "complexity": {},
        "text_lengths": [],
        "ir_lengths": [],
    }

    for example in dataset:
        # Count by family
        family = example.get("family", "unknown")
        stats["families"][family] = stats["families"].get(family, 0) + 1

        # Count by complexity
        complexity = example.get("complexity", 0)
        stats["complexity"][complexity] = stats["complexity"].get(complexity, 0) + 1

        # Track lengths
        stats["text_lengths"].append(len(example.get("text", "")))
        stats["ir_lengths"].append(len(example.get("ir", "")))

    # Compute length statistics
    if stats["text_lengths"]:
        stats["text_length_stats"] = {
            "min": min(stats["text_lengths"]),
            "max": max(stats["text_lengths"]),
            "mean": sum(stats["text_lengths"]) / len(stats["text_lengths"]),
        }
        stats["ir_length_stats"] = {
            "min": min(stats["ir_lengths"]),
            "max": max(stats["ir_lengths"]),
            "mean": sum(stats["ir_lengths"]) / len(stats["ir_lengths"]),
        }

    # Remove raw length lists from output
    del stats["text_lengths"]
    del stats["ir_lengths"]

    return stats
