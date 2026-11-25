"""The whole K01 measurement in one command: render the classroom cameras with gltf_viewer and compare them with the Cycles references.

    python tools/scenes/compare_all.py                      # builds the viewer, renders 3 cameras x 2 lightings, compares with reference-spot/ and reference/
    python tools/scenes/compare_all.py --references reference-spot --lightings lights-only --supersample 1 --out-dir out/k01

Needs the references of tools/scenes/render_reference.py in test-assets/classroom/ (git-ignored), numpy and Pillow;
`flip_evaluator` is used by compare.py when the interpreter has it. Writes <out-dir>/<reference>/<lighting>/<camera>_{metrics.json,collage.png,delta_e.png},
the images <out-dir>/pathon/<lighting>_<camera>.npy and <out-dir>/summary.md / summary.json (docs/porownanie-obrazu.md).
"""

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[2]
COMPARE = Path(__file__).with_name("compare.py")
CAMERAS = ("renderCam", "pathon_wide", "pathon_reverse")
SUMMARY_COLUMNS = (
    ("PSNR dB", lambda m: m["psnr_db"], "{:.1f}"),
    ("SSIM", lambda m: m["ssim"], "{:.3f}"),
    ("dE2000 mean", lambda m: m["delta_e2000_mean"], "{:.1f}"),
    ("dE2000 p95", lambda m: m["delta_e2000_p95"], "{:.1f}"),
    ("FLIP LDR", lambda m: m["flip"]["ldr_mean"] if isinstance(m["flip"], dict) else None, "{:.3f}"),
    ("FLIP HDR", lambda m: m["flip"]["hdr_mean"] if isinstance(m["flip"], dict) else None, "{:.3f}"),
    ("bias", lambda m: m["bias_total"], "{:.2f}"),
    ("bias w/o windows", lambda m: m.get("bias_without_windows"), "{:.2f}"),
)


def viewer_binary():
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    return target / "release" / ("gltf_viewer.exe" if os.name == "nt" else "gltf_viewer")


def run(command, **kwargs):
    result = subprocess.run([str(part) for part in command], cwd=ROOT, **kwargs)
    if result.returncode != 0:
        sys.exit(f"failed ({result.returncode}): {' '.join(str(part) for part in command)}")


def render_pathon(args, lighting, camera, output):
    run([viewer_binary(), args.scene, "--camera-name", camera, "--reference-like", lighting, "--dump-hdr", output,
         "--size", "960x540", "--supersample", args.supersample, "--backend", args.backend])


def docs_collage(collage_path, output, title):
    """The 4-panel collage of compare.py (reference | pathon | dE2000 | luminance ratio) as a 2x2 picture of 960x540 for docs/."""
    sheet = Image.open(collage_path).convert("RGB")
    width, height = sheet.width // 4, sheet.height
    panels = [sheet.crop((i * width, 0, (i + 1) * width, height)).resize((480, 270), Image.LANCZOS) for i in range(4)]
    out = Image.new("RGB", (960, 540))
    for i, (panel, label) in enumerate(zip(panels, ("Cycles (denoised)", "pathon", "dE2000 (yellow = 20)", "pathon / Cycles (red brighter)"))):
        draw = ImageDraw.Draw(panel)
        draw.rectangle((0, 0, 8 * len(label) + 8, 14), fill=(0, 0, 0))
        draw.text((4, 1), label, fill=(255, 255, 255))
        out.paste(panel, ((i % 2) * 480, (i // 2) * 270))
    ImageDraw.Draw(out).text((4, 528), title, fill=(255, 255, 255))
    output.parent.mkdir(parents=True, exist_ok=True)
    out.save(output, optimize=True)


def format_cell(value, template):
    return "-" if value is None else template.format(value)


def summary_values(metrics):
    return [getter(metrics) for _, getter, _ in SUMMARY_COLUMNS]


def summary_table(rows):
    """rows: ((reference, lighting, camera), [value per SUMMARY_COLUMNS])."""
    header = "| Reference | Lighting | Camera | " + " | ".join(name for name, _, _ in SUMMARY_COLUMNS) + " |"
    lines = [header, "| --- | --- | --- |" + " ---: |" * len(SUMMARY_COLUMNS)]
    for (reference, lighting, camera), values in rows:
        cells = [format_cell(value, template) for value, (_, _, template) in zip(values, SUMMARY_COLUMNS)]
        lines.append(f"| {reference} | {lighting} | {camera} | " + " | ".join(cells) + " |")
    return "\n".join(lines)


def mean_rows(rows):
    """One row per reference and lighting: the mean of the cameras, the way the thresholds are stated."""
    groups = {}
    for (reference, lighting, _), metrics in rows:
        groups.setdefault((reference, lighting), []).append(summary_values(metrics))
    averaged = []
    for (reference, lighting), group in groups.items():
        columns = [[v for v in column if v is not None] for column in zip(*group)]
        averaged.append(((reference, lighting, "mean"), [sum(c) / len(c) if c else None for c in columns]))
    return averaged


def mask_table(rows):
    masks = sorted({name for _, metrics in rows for name in metrics["per_mask"]})
    lines = ["| Reference | Lighting | Camera | " + " | ".join(masks) + " |", "| --- | --- | --- |" + " ---: |" * len(masks)]
    for (reference, lighting, camera), metrics in rows:
        cells = []
        for mask in masks:
            entry = metrics["per_mask"].get(mask, {})
            cells.append(format_cell(entry.get("bias_linear"), "{:.2f}"))
        lines.append(f"| {reference} | {lighting} | {camera} | " + " | ".join(cells) + " |")
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(prog="compare_all.py", description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--scene", default="test-assets/classroom/classroom.glb")
    parser.add_argument("--classroom", default="test-assets/classroom", help="folder with the reference directories")
    parser.add_argument("--references", default="reference-spot,reference")
    parser.add_argument("--lightings", default="lights-only,ibl")
    parser.add_argument("--cameras", default=",".join(CAMERAS))
    parser.add_argument("--supersample", default="2", help="box average of N x N samples per pixel (the Cycles pixel filter is a 1 px box)")
    parser.add_argument("--backend", default="vulkan")
    parser.add_argument("--out-dir", default="out/k01")
    parser.add_argument("--no-build", action="store_true", help="use the gltf_viewer binary that is already built")
    parser.add_argument("--no-render", action="store_true", help="reuse the .npy files of an earlier run")
    parser.add_argument("--docs-dir", default=None, help="also write 960x540 collages for docs/scenes/reference/ here")
    parser.add_argument("--docs-select", default="reference-spot/lights-only,reference/lights-only", help="reference/lighting pairs that get a docs collage")
    parser.add_argument("--raw", action="store_true", help="compare with the raw (not denoised) reference")
    args = parser.parse_args()
    out_dir = ROOT / args.out_dir
    references, lightings, cameras = (part.split(",") for part in (args.references, args.lightings, args.cameras))

    if not args.no_render and not args.no_build:
        run(["cargo", "build", "--release", "-p", "pathon-examples", "--bin", "gltf_viewer"])
    started = time.time()
    (out_dir / "pathon").mkdir(parents=True, exist_ok=True)
    rows = []
    for lighting in lightings:
        for camera in cameras:
            image = out_dir / "pathon" / f"{lighting}_{camera}.npy"
            if not args.no_render:
                render_pathon(args, lighting, camera, image)
            for reference in references:
                suffix = "" if args.raw else "_denoised"
                reference_image = ROOT / args.classroom / reference / lighting / f"{camera}{suffix}.exr"
                target = out_dir / reference / lighting
                run([sys.executable, COMPARE, "--reference", reference_image, "--test", image, "--out-dir", target, "--name", camera],
                    stdout=subprocess.DEVNULL)
                rows.append(((reference, lighting, camera), json.loads((target / f"{camera}_metrics.json").read_text(encoding="utf-8"))))
                if args.docs_dir and f"{reference}/{lighting}" in args.docs_select.split(","):
                    docs_collage(target / f"{camera}_collage.png", Path(args.docs_dir) / f"compare-{reference}-{lighting}-{camera}.png",
                                 f"{reference} / {lighting} / {camera}")

    text = ("## Per camera\n\n" + summary_table([(key, summary_values(m)) for key, m in rows])
            + "\n\n## Mean of the cameras\n\n" + summary_table(mean_rows(rows))
            + "\n\n## Bias of the luminance per mask (pathon / reference)\n\n" + mask_table(rows) + "\n")
    (out_dir / "summary.md").write_text(text, encoding="utf-8")
    (out_dir / "summary.json").write_text(json.dumps({"|".join(key): metrics for key, metrics in rows}, indent=1), encoding="utf-8")
    print(text)
    print(f"done in {time.time() - started:.0f} s; collages and metrics in {out_dir}")


if __name__ == "__main__":
    main()
