"""Direct light of pathon against the direct-light AOV terms of the Cycles reference, in the half of the pixels where the
reference has the least direct light (mask direct_shadowed) and in the other half (direct_lit).

    python tools/scenes/direct_ratio.py out/k03-before out/k03-after [--reference reference-spot] [--lighting lights-only]

Every argument is an output folder of compare_all.py (it holds pathon/<lighting>_<camera>.npy). pathon is rendered with
`--reference-like lights-only`, so the image is direct light only and the ratio is a pure comparison of direct lighting.
The AOV terms come from the raw (not denoised) EXR of the reference. Source 3 of the "five biggest sources" table of
docs/porownanie-obrazu.md and the K03 section use it.
"""

import argparse
import importlib.util
import json
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
CAMERAS = ("renderCam", "pathon_wide", "pathon_reverse")


def load_compare():
    spec = importlib.util.spec_from_file_location("compare", Path(__file__).with_name("compare.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main():
    parser = argparse.ArgumentParser(prog="direct_ratio.py", description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("runs", nargs="+", help="output folders of compare_all.py")
    parser.add_argument("--classroom", default="test-assets/classroom")
    parser.add_argument("--reference", default="reference", help="reference folder: reference or reference-spot")
    parser.add_argument("--lighting", default="lights-only")
    args = parser.parse_args()
    compare = load_compare()
    classroom = ROOT / args.classroom
    material_ids = json.loads((classroom / args.reference / "reference.json").read_text(encoding="utf-8"))["material_ids"]
    print(f"| {args.reference} / {args.lighting} | " + " | ".join(CAMERAS) + " |")
    print("| --- |" + " ---: |" * len(CAMERAS))
    rows = {run: [] for run in args.runs}
    for camera in CAMERAS:
        layers = compare.read_exr_layers(classroom / args.reference / args.lighting / f"{camera}.exr")
        direct = compare.luminance(compare.direct_only_image(layers))
        masks = compare.build_masks(layers, material_ids)
        for run in args.runs:
            image = compare.luminance(np.load(ROOT / run / "pathon" / f"{args.lighting}_{camera}.npy"))
            shadowed, lit = masks["direct_shadowed"], masks["direct_lit"]
            ratio = lambda mask: image[mask].mean() / direct[mask].mean()
            rows[run].append((ratio(shadowed), ratio(lit)))
    for run, values in rows.items():
        print(f"| {run}, shadowed half | " + " | ".join(f"{shadowed:.2f}" for shadowed, _ in values) + " |")
        print(f"| {run}, lit half | " + " | ".join(f"{lit:.2f}" for _, lit in values) + " |")


if __name__ == "__main__":
    main()
