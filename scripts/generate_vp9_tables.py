#!/usr/bin/env python3
"""Generate VP9 probability tables from the reference MPP C source."""

from __future__ import annotations

import argparse
import ast
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
DEFAULT_SOURCE = ROOT / "refs/mpp/mpp/codec/dec/vp9/vp9d_codec.c"
DEFAULT_HAL_SOURCE = ROOT / "refs/mpp/mpp/hal/rkdec/vp9d/hal_vp9d_com.c"
DEFAULT_OUTPUT = Path(__file__).resolve().parents[1] / "src/codec/vp9/tables.rs"


def strip_c_comments(source: str) -> str:
    result: list[str] = []
    index = 0
    while index < len(source):
        if source.startswith("//", index):
            newline = source.find("\n", index)
            index = len(source) if newline < 0 else newline
        elif source.startswith("/*", index):
            end = source.find("*/", index + 2)
            if end < 0:
                raise ValueError("unterminated C block comment")
            index = end + 2
            result.append(" ")
        elif source[index] in {'"', "'"}:
            quote = source[index]
            result.append(source[index])
            index += 1
            while index < len(source):
                result.append(source[index])
                if source[index] == "\\":
                    index += 1
                    if index < len(source):
                        result.append(source[index])
                elif source[index] == quote:
                    index += 1
                    break
                index += 1
        else:
            result.append(source[index])
            index += 1
    return "".join(result)


def extract_initializer(source: str, declaration: str) -> list:
    start = source.find(declaration)
    if start < 0:
        raise ValueError(f"declaration not found: {declaration}")
    start = source.find("{", source.find("=", start))
    if start < 0:
        raise ValueError(f"initializer not found: {declaration}")

    depth = 0
    for end in range(start, len(source)):
        if source[end] == "{":
            depth += 1
        elif source[end] == "}":
            depth -= 1
            if depth == 0:
                initializer = source[start : end + 1]
                initializer = initializer.replace("{", "[").replace("}", "]")
                return ast.literal_eval(initializer)
    raise ValueError(f"unclosed initializer: {declaration}")


def shape(value: list) -> tuple[int, ...]:
    dimensions = []
    while isinstance(value, list):
        dimensions.append(len(value))
        if not value:
            break
        value = value[0]
    return tuple(dimensions)


def expect_shape(value: list, expected: tuple[int, ...], label: str) -> None:
    actual = shape(value)
    if actual != expected:
        raise ValueError(f"{label} shape is {actual}, expected {expected}")


def rust_array(value: list | int, indent: int = 0) -> str:
    if isinstance(value, int):
        return str(value)
    if not value:
        return "[]"
    next_indent = indent + 4
    items = [rust_array(item, next_indent) for item in value]
    return "[\n" + "".join(
        " " * next_indent + item + ",\n" for item in items
    ) + " " * indent + "]"


def render(source_path: Path, hal_source_path: Path, output_path: Path) -> None:
    source = strip_c_comments(source_path.read_text())
    hal_source = strip_c_comments(hal_source_path.read_text())
    header = extract_initializer(source, "const Vp9ProbCtx vp9_default_probs")
    coefficients = extract_initializer(
        source, "const RK_U8 vp9_default_coef_probs"
    )
    keyframe_y_mode = extract_initializer(
        hal_source, "const vp9_prob vp9_kf_y_mode_prob"
    )
    keyframe_uv_mode = extract_initializer(
        hal_source, "const vp9_prob vp9_kf_uv_mode_prob"
    )
    keyframe_partition = extract_initializer(
        hal_source, "const vp9_prob vp9_kf_partition_probs"
    )
    if len(header) != 15:
        raise ValueError(f"vp9_default_probs has {len(header)} fields, expected 15")
    expect_shape(coefficients, (4, 2, 2, 6, 6, 3), "vp9_default_coef_probs")
    expect_shape(keyframe_y_mode, (10, 10, 9), "vp9_kf_y_mode_prob")
    expect_shape(keyframe_uv_mode, (10, 9), "vp9_kf_uv_mode_prob")
    expect_shape(keyframe_partition, (16, 3), "vp9_kf_partition_probs")

    (
        y_mode,
        uv_mode,
        filter_,
        mv_mode,
        intra,
        comp,
        single_ref,
        comp_ref,
        tx32p,
        tx16p,
        tx8p,
        skip,
        mv_joint,
        mv_comp,
        partition,
    ) = header
    expected_fields = {
        "y_mode": (y_mode, (4, 9)),
        "uv_mode": (uv_mode, (10, 9)),
        "filter": (filter_, (4, 2)),
        "mv_mode": (mv_mode, (7, 3)),
        "intra": (intra, (4,)),
        "comp": (comp, (5,)),
        "single_ref": (single_ref, (5, 2)),
        "comp_ref": (comp_ref, (5,)),
        "tx32p": (tx32p, (2, 3)),
        "tx16p": (tx16p, (2, 2)),
        "tx8p": (tx8p, (2,)),
        "skip": (skip, (3,)),
        "mv_joint": (mv_joint, (3,)),
        "mv_comp": (mv_comp, (2, 8)),
        "partition": (partition, (4, 4, 3)),
    }
    for name, (value, expected) in expected_fields.items():
        expect_shape(value, expected, name)
    for index in (0, 1):
        component = mv_comp[index]
        expect_shape(component[1], (10,), f"mv_comp[{index}].classes")
        expect_shape(component[3], (10,), f"mv_comp[{index}].bits")
        expect_shape(component[4], (2, 3), f"mv_comp[{index}].class0_fp")
        expect_shape(component[5], (3,), f"mv_comp[{index}].fp")

    components = []
    for component in mv_comp:
        sign, classes, class0, bits, class0_fp, fp, class0_hp, hp = component
        components.append(
            "    MotionVectorProbabilities {\n"
            f"        sign: {sign},\n"
            f"        classes: {rust_array(classes, 8)},\n"
            f"        class0: {class0},\n"
            f"        bits: {rust_array(bits, 8)},\n"
            f"        class0_fp: {rust_array(class0_fp, 8)},\n"
            f"        fp: {rust_array(fp, 8)},\n"
            f"        class0_hp: {class0_hp},\n"
            f"        hp: {hp},\n"
            "    }"
        )

    output = f"""// @generated by scripts/generate_vp9_tables.py from
// refs/mpp/mpp/codec/dec/vp9/vp9d_codec.c. Do not edit by hand.

use super::prob::{{CoefficientProbabilities, HeaderProbabilities, MotionVectorProbabilities}};

pub const DEFAULT_HEADER_PROBS: HeaderProbabilities = HeaderProbabilities {{
    y_mode: {rust_array(y_mode, 4)},
    uv_mode: {rust_array(uv_mode, 4)},
    filter: {rust_array(filter_, 4)},
    mv_mode: {rust_array(mv_mode, 4)},
    intra: {rust_array(intra, 4)},
    comp: {rust_array(comp, 4)},
    single_ref: {rust_array(single_ref, 4)},
    comp_ref: {rust_array(comp_ref, 4)},
    tx32p: {rust_array(tx32p, 4)},
    tx16p: {rust_array(tx16p, 4)},
    tx8p: {rust_array(tx8p, 4)},
    skip: {rust_array(skip, 4)},
    mv_joint: {rust_array(mv_joint, 4)},
    mv_comp: [
{",".join(components).replace(chr(10), chr(10) + "    ")}
    ],
    partition: {rust_array(partition, 4)},
}};

pub const DEFAULT_COEF_PROBS: CoefficientProbabilities = {rust_array(coefficients)};

pub const KEYFRAME_Y_MODE_PROBS: [[[u8; 9]; 10]; 10] = {rust_array(keyframe_y_mode)};

pub const KEYFRAME_UV_MODE_PROBS: [[u8; 9]; 10] = {rust_array(keyframe_uv_mode)};

pub const KEYFRAME_PARTITION_PROBS: [[u8; 3]; 16] = {rust_array(keyframe_partition)};
"""
    output_path.write_text(output)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", type=Path, default=DEFAULT_SOURCE)
    parser.add_argument("--hal-source", type=Path, default=DEFAULT_HAL_SOURCE)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    args = parser.parse_args()
    render(args.source, args.hal_source, args.output)


if __name__ == "__main__":
    main()
