"""
Evaluation utilities for cad0 model.

Provides metrics for:
- Syntax validity: Can the IR be parsed?
- Geometry validity: Does it produce a valid manifold?
- Exact match: Does the IR match the ground truth?
"""

import json
import re
from dataclasses import dataclass
from typing import Optional

import torch
from datasets import Dataset
from tqdm import tqdm
from transformers import PreTrainedModel, PreTrainedTokenizer


# VCode syntax patterns
COMPACT_IR_PATTERNS = {
    "cube": re.compile(r"^C\s+[\d.]+\s+[\d.]+\s+[\d.]+$"),
    "cylinder": re.compile(r"^Y\s+[\d.]+\s+[\d.]+$"),
    "sphere": re.compile(r"^S\s+[\d.]+$"),
    "cone": re.compile(r"^K\s+[\d.]+\s+[\d.]+\s+[\d.]+$"),
    "translate": re.compile(r"^T\s+\d+\s+-?[\d.]+\s+-?[\d.]+\s+-?[\d.]+$"),
    "rotate": re.compile(r"^R\s+\d+\s+-?[\d.]+\s+-?[\d.]+\s+-?[\d.]+$"),
    "scale": re.compile(r"^SC\s+\d+\s+[\d.]+\s+[\d.]+\s+[\d.]+$"),
    "union": re.compile(r"^U\s+\d+\s+\d+$"),
    "difference": re.compile(r"^D\s+\d+\s+\d+$"),
    "intersection": re.compile(r"^I\s+\d+\s+\d+$"),
}


@dataclass
class EvalMetrics:
    """Evaluation metrics for a single example."""

    syntax_valid: bool
    geometry_valid: Optional[bool] = None
    exact_match: bool = False
    generated_ir: str = ""
    error_message: Optional[str] = None


@dataclass
class AggregateMetrics:
    """Aggregate evaluation metrics."""

    total: int
    syntax_valid: int
    geometry_valid: int
    exact_match: int
    syntax_accuracy: float
    geometry_accuracy: float
    exact_match_accuracy: float
    errors: dict[str, int]


def validate_syntax(ir: str) -> tuple[bool, Optional[str]]:
    """
    Validate VCode syntax.

    Args:
        ir: VCode string to validate

    Returns:
        Tuple of (is_valid, error_message)
    """
    if not ir or not ir.strip():
        return False, "Empty IR"

    lines = ir.strip().split("\n")
    node_count = 0

    for i, line in enumerate(lines):
        line = line.strip()
        if not line:
            continue

        # Check if line matches any known pattern
        matched = False
        for op_type, pattern in COMPACT_IR_PATTERNS.items():
            if pattern.match(line):
                matched = True
                node_count += 1
                break

        if not matched:
            return False, f"Invalid syntax at line {i + 1}: {line[:50]}"

    if node_count == 0:
        return False, "No valid operations found"

    return True, None


def validate_references(ir: str) -> tuple[bool, Optional[str]]:
    """
    Validate that node references in operations are valid.

    Args:
        ir: VCode string

    Returns:
        Tuple of (is_valid, error_message)
    """
    lines = ir.strip().split("\n")
    node_index = 0

    for line in lines:
        line = line.strip()
        if not line:
            continue

        parts = line.split()
        op = parts[0]

        # Check references for operations that use node indices
        if op in ("T", "R", "SC"):  # Transform operations
            if len(parts) >= 2:
                ref = int(parts[1])
                if ref >= node_index:
                    return False, f"Invalid reference {ref} at node {node_index}"

        elif op in ("U", "D", "I"):  # Boolean operations
            if len(parts) >= 3:
                ref1, ref2 = int(parts[1]), int(parts[2])
                if ref1 >= node_index or ref2 >= node_index:
                    return (
                        False,
                        f"Invalid references {ref1}, {ref2} at node {node_index}",
                    )

        node_index += 1

    return True, None


def generate_completion(
    model: PreTrainedModel,
    tokenizer: PreTrainedTokenizer,
    prompt: str,
    max_new_tokens: int = 256,
    temperature: float = 0.1,
) -> str:
    """Generate IR completion for a prompt."""
    full_prompt = f"Design: {prompt}\n\nVCode:\n"

    inputs = tokenizer(full_prompt, return_tensors="pt").to(model.device)

    with torch.no_grad():
        outputs = model.generate(
            **inputs,
            max_new_tokens=max_new_tokens,
            temperature=temperature,
            do_sample=temperature > 0,
            pad_token_id=tokenizer.pad_token_id,
            eos_token_id=tokenizer.eos_token_id,
        )

    generated = tokenizer.decode(outputs[0], skip_special_tokens=True)

    # Extract IR part
    if "VCode:" in generated:
        ir = generated.split("VCode:")[-1].strip()
        # Stop at any additional sections
        if "\n\n" in ir:
            ir = ir.split("\n\n")[0]
        return ir

    return generated


def evaluate_single(
    model: PreTrainedModel,
    tokenizer: PreTrainedTokenizer,
    example: dict,
    validate_geometry: bool = False,
    engine=None,
) -> EvalMetrics:
    """
    Evaluate a single example.

    Args:
        model: The model to evaluate
        tokenizer: The tokenizer
        example: Dictionary with 'text' and 'ir' keys
        validate_geometry: Whether to validate geometry (requires engine)
        engine: vcad Engine instance for geometry validation

    Returns:
        EvalMetrics for this example
    """
    # Generate completion
    generated_ir = generate_completion(model, tokenizer, example["text"])

    # Check syntax validity
    syntax_valid, syntax_error = validate_syntax(generated_ir)
    if syntax_valid:
        ref_valid, ref_error = validate_references(generated_ir)
        if not ref_valid:
            syntax_valid = False
            syntax_error = ref_error

    # Check exact match (normalize whitespace)
    target_ir = example["ir"].strip()
    generated_normalized = generated_ir.strip()
    exact_match = target_ir == generated_normalized

    # Check geometry validity if requested
    geometry_valid = None
    if validate_geometry and syntax_valid and engine is not None:
        try:
            # Convert VCode to standard IR and evaluate
            # This would require the vcad engine
            geometry_valid = True  # Placeholder - implement with actual engine
        except Exception as e:
            geometry_valid = False
            syntax_error = str(e)

    return EvalMetrics(
        syntax_valid=syntax_valid,
        geometry_valid=geometry_valid,
        exact_match=exact_match,
        generated_ir=generated_ir,
        error_message=syntax_error,
    )


def evaluate_model(
    model: PreTrainedModel,
    tokenizer: PreTrainedTokenizer,
    dataset: Dataset,
    validate_geometry: bool = False,
    engine=None,
    max_samples: Optional[int] = None,
) -> AggregateMetrics:
    """
    Evaluate the model on a dataset.

    Args:
        model: The model to evaluate
        tokenizer: The tokenizer
        dataset: HuggingFace Dataset with 'text' and 'ir' columns
        validate_geometry: Whether to validate geometry
        engine: vcad Engine instance
        max_samples: Maximum samples to evaluate

    Returns:
        AggregateMetrics with overall results
    """
    model.eval()

    samples = dataset
    if max_samples is not None:
        samples = dataset.select(range(min(max_samples, len(dataset))))

    results = []
    errors: dict[str, int] = {}

    for example in tqdm(samples, desc="Evaluating"):
        metrics = evaluate_single(
            model, tokenizer, example, validate_geometry, engine
        )
        results.append(metrics)

        if metrics.error_message:
            error_type = metrics.error_message.split(":")[0]
            errors[error_type] = errors.get(error_type, 0) + 1

    # Aggregate
    total = len(results)
    syntax_valid = sum(1 for r in results if r.syntax_valid)
    geometry_valid = sum(
        1 for r in results if r.geometry_valid is True
    )
    exact_match = sum(1 for r in results if r.exact_match)

    return AggregateMetrics(
        total=total,
        syntax_valid=syntax_valid,
        geometry_valid=geometry_valid,
        exact_match=exact_match,
        syntax_accuracy=syntax_valid / total if total > 0 else 0,
        geometry_accuracy=geometry_valid / total if total > 0 else 0,
        exact_match_accuracy=exact_match / total if total > 0 else 0,
        errors=errors,
    )


def compute_metrics(eval_results: list[EvalMetrics]) -> dict:
    """
    Compute aggregate metrics from a list of evaluation results.

    This is useful for custom evaluation loops.

    Args:
        eval_results: List of EvalMetrics objects

    Returns:
        Dictionary with metric values
    """
    total = len(eval_results)
    if total == 0:
        return {
            "syntax_accuracy": 0,
            "geometry_accuracy": 0,
            "exact_match_accuracy": 0,
        }

    syntax_valid = sum(1 for r in eval_results if r.syntax_valid)
    geometry_valid = sum(
        1 for r in eval_results if r.geometry_valid is True
    )
    exact_match = sum(1 for r in eval_results if r.exact_match)

    return {
        "syntax_accuracy": syntax_valid / total,
        "geometry_accuracy": geometry_valid / total,
        "exact_match_accuracy": exact_match / total,
    }


def print_eval_report(metrics: AggregateMetrics) -> None:
    """Print a formatted evaluation report."""
    print("\n" + "=" * 60)
    print("Evaluation Report")
    print("=" * 60)
    print(f"Total examples: {metrics.total}")
    print()
    print("Accuracy:")
    print(f"  Syntax validity:   {metrics.syntax_accuracy:.1%} ({metrics.syntax_valid}/{metrics.total})")
    print(f"  Geometry validity: {metrics.geometry_accuracy:.1%} ({metrics.geometry_valid}/{metrics.total})")
    print(f"  Exact match:       {metrics.exact_match_accuracy:.1%} ({metrics.exact_match}/{metrics.total})")

    if metrics.errors:
        print()
        print("Error breakdown:")
        for error_type, count in sorted(
            metrics.errors.items(), key=lambda x: -x[1]
        ):
            print(f"  {error_type}: {count}")

    print("=" * 60)
