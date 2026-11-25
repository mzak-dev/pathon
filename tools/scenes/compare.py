"""Compares a pathon image with a Cycles reference (docs/porownanie-obrazu.md).

    python tools/scenes/compare.py --reference test-assets/classroom/reference/lights-only/renderCam.exr \
        --test pathon_renderCam.npy --out-dir out/renderCam [--name renderCam] [--test-scale 1.0] \
        [--exposure 1.0] [--reference-json test-assets/classroom/reference/reference.json]

    python tools/scenes/compare.py --reference <exr> --synthetic direct-only|direct-flat-ambient --out-dir out   # reference vs its own AOV subset

Inputs are linear Rec.709 radiance (.npy float32 HxWx3, .exr, Radiance .hdr) or sRGB 8-bit .png (LDR only, so the
bias and the energy metrics lose meaning). Needs numpy and Pillow; `flip_evaluator` (BSD-3, pip) is used when installed.
Both images go through the same PBR Neutral tonemap and the same sRGB encoding before the perceptual metrics.
A `<camera>_denoised.exr` reference takes masks and AOV shares from the `<camera>.exr` next to it.
Writes <name>_metrics.json, <name>_collage.png (reference | test | DeltaE2000 heat map | log2 luminance ratio).
"""

import argparse
import json
import re
import struct
import sys
import zlib
from pathlib import Path

import numpy as np
from PIL import Image

LUMA = np.array([0.2126, 0.7152, 0.0722], dtype=np.float32)

# Material groups by name for the Material Index pass; regexes are matched against the material names of reference.json.
DEFAULT_MATERIAL_MASKS = {
    "board": r"^(blackBoard|whiteChalk|boardFrame)$",
    "floor": r"^woodFloor$",
    "windows": r"^(whitePaintedWindow|paintedBlind)$",
    "walls_ceiling": r"^(beigeWall|plaster|paintedCeiling|greyDiffusePaint|beigePaint)$",
    "wood_furniture": r"^(schoolDesk_wood|teacherDesk_wood|woodPlanks.*|varnishedWoodDoor.*|suitCoat_darkWood)$",
    "metal": r"(metal|brass)",
    "plastic_desk_tops": r"^(schoolDesk_plastic|teacherDesk_plastic)$",
}


# --------------------------------------------------------------------------------------------
# Readers: EXR (scanline, NONE/ZIPS/ZIP, half and float), Radiance HDR, npy, PNG
# --------------------------------------------------------------------------------------------

def read_exr_layers(path):
    """All channels of an uncompressed or ZIP/ZIPS scanline OpenEXR file as {name: float32 array (H, W)}."""
    data = Path(path).read_bytes()
    magic, flags = struct.unpack_from("<II", data, 0)
    if magic != 20000630:
        raise ValueError(f"{path}: not an OpenEXR file")
    multipart = bool(flags & 0x1000)
    position, headers = 8, []
    while True:
        header = {}
        while data[position] != 0:
            end = data.index(b"\0", position)
            name = data[position:end].decode()
            end2 = data.index(b"\0", end + 1)
            size = struct.unpack_from("<I", data, end2 + 1)[0]
            header[name] = data[end2 + 5:end2 + 5 + size]
            position = end2 + 5 + size
        position += 1
        if not multipart or not header:
            break
        headers.append(header)
    if not multipart:
        headers = [header]
    layers = {}
    tables = []
    for header in headers:
        count = struct.unpack("<i", header["chunkCount"])[0] if multipart else None
        block_lines = {0: 1, 2: 1, 3: 16}.get(header["compression"][0])
        x_min, y_min, x_max, y_max = struct.unpack("<4i", header["dataWindow"])
        count = count if count is not None else ((y_max - y_min + 1) + block_lines - 1) // block_lines
        tables.append(struct.unpack_from(f"<{count}Q", data, position))
        position += 8 * count
    for header, offsets in zip(headers, tables):
        layers.update(decode_part(path, data, header, offsets, multipart))
    return layers


def decode_part(path, data, header, offsets, multipart):
    channels, cursor = [], 0
    chlist = header["channels"]
    while chlist[cursor] != 0:
        end = chlist.index(b"\0", cursor)
        name = chlist[cursor:end].decode()
        pixel_type = struct.unpack_from("<I", chlist, end + 1)[0]
        channels.append((name, pixel_type))
        cursor = end + 1 + 16
    x_min, y_min, x_max, y_max = struct.unpack("<4i", header["dataWindow"])
    width, height = x_max - x_min + 1, y_max - y_min + 1
    compression = header["compression"][0]
    lines_per_block = {0: 1, 2: 1, 3: 16}.get(compression)
    if lines_per_block is None:
        raise ValueError(f"{path}: compression {compression} unsupported (use ZIP, ZIPS or NONE)")
    sizes = {0: 4, 1: 2, 2: 4}
    planes = {name: np.empty((height, width), np.float32) for name, _ in channels}
    chunk_prefix = 4 if multipart else 0  # multipart chunks start with the part number
    for offset in offsets:
        y, packed_size = struct.unpack_from("<iI", data, offset + chunk_prefix)
        payload = data[offset + chunk_prefix + 8:offset + chunk_prefix + 8 + packed_size]
        lines = min(lines_per_block, height - (y - y_min))
        expected = lines * width * sum(sizes[t] for _, t in channels)
        raw = payload if compression == 0 or packed_size == expected else undo_zip(payload)
        view = np.frombuffer(raw, np.uint8)
        cursor = 0
        for line in range(lines):
            for name, pixel_type in channels:
                count = width * sizes[pixel_type]
                chunk = view[cursor:cursor + count]
                cursor += count
                dtype = np.float16 if pixel_type == 1 else np.float32
                planes[name][y - y_min + line] = chunk.view(dtype)
    return planes


def undo_zip(payload):
    """OpenEXR ZIP: inflate, undo the delta predictor, then re-interleave the two halves of the bytes."""
    stream = np.frombuffer(zlib.decompress(payload), np.uint8).astype(np.int16)
    stream[1:] = stream[1:] - 128
    stream = (np.cumsum(stream) & 0xFF).astype(np.uint8)
    half = (len(stream) + 1) // 2
    out = np.empty(len(stream), np.uint8)
    out[0::2] = stream[:half]
    out[1::2] = stream[half:]
    return out.tobytes()


def read_radiance_hdr(path):
    data = Path(path).read_bytes()
    end = data.index(b"\n\n") + 2
    line_end = data.index(b"\n", end)
    match = re.match(rb"-Y (\d+) \+X (\d+)", data[end:line_end])
    height, width = int(match.group(1)), int(match.group(2))
    cursor, out = line_end + 1, np.empty((height, width, 4), np.uint8)
    for y in range(height):
        if data[cursor:cursor + 2] == b"\x02\x02" and (data[cursor + 2] << 8 | data[cursor + 3]) == width:
            cursor += 4
            for channel in range(4):
                x = 0
                while x < width:
                    count = data[cursor]
                    cursor += 1
                    if count > 128:
                        count -= 128
                        out[y, x:x + count, channel] = data[cursor]
                        cursor += 1
                    else:
                        out[y, x:x + count, channel] = np.frombuffer(data, np.uint8, count, cursor)
                        cursor += count
                    x += count
        else:
            out[y] = np.frombuffer(data, np.uint8, width * 4, cursor).reshape(width, 4)
            cursor += width * 4
    scale = np.where(out[..., 3] > 0, np.ldexp(1.0, out[..., 3].astype(np.int32) - 136), 0.0)
    return (out[..., :3] * scale[..., None]).astype(np.float32)


def srgb_to_linear(value):
    return np.where(value <= 0.04045, value / 12.92, ((value + 0.055) / 1.055) ** 2.4)


def linear_to_srgb(value):
    value = np.clip(value, 0.0, 1.0)
    return np.where(value <= 0.0031308, value * 12.92, 1.055 * value ** (1 / 2.4) - 0.055)


def exr_rgb(layers, prefix):
    return np.stack([layers[f"{prefix}.{c}"] for c in "RGB"], axis=-1)


def pick_combined(layers):
    for prefix in ("ViewLayer.Combined", "Combined", ""):
        if (f"{prefix}.R" if prefix else "R") in layers:
            return exr_rgb(layers, prefix) if prefix else np.stack([layers[c] for c in "RGB"], axis=-1)
    raise ValueError("no Combined/RGB channels in the EXR file")


def load_radiance(path):
    """Linear RGB float32 (H, W, 3) and, for multilayer EXR, the layers dictionary."""
    suffix = Path(path).suffix.lower()
    if suffix == ".exr":
        layers = read_exr_layers(path)
        return pick_combined(layers), layers
    if suffix == ".npy":
        return np.load(path).astype(np.float32)[..., :3], None
    if suffix == ".hdr":
        return read_radiance_hdr(path), None
    if suffix == ".png":
        pixels = np.asarray(Image.open(path).convert("RGB"), dtype=np.float32) / 255.0
        return srgb_to_linear(pixels).astype(np.float32), None
    raise ValueError(f"unsupported file type: {path}")


# --------------------------------------------------------------------------------------------
# Tonemap, colour, metrics
# --------------------------------------------------------------------------------------------

def tonemap_pbr_neutral(rgb):
    """Khronos PBR Neutral; the same operator for both images so that operator differences do not enter the score."""
    start_compression, desaturation = 0.8 - 0.04, 0.15
    smallest = rgb.min(axis=-1)
    offset = np.where(smallest < 0.08, smallest - 6.25 * smallest * smallest, 0.04)
    rgb = rgb - offset[..., None]
    peak = rgb.max(axis=-1)
    d = 1.0 - start_compression
    new_peak = 1.0 - d * d / (peak + d - start_compression)
    compressed = rgb * (new_peak / np.maximum(peak, 1e-8))[..., None]
    mix = 1.0 - 1.0 / (desaturation * (peak - new_peak) + 1.0)
    compressed = compressed + (new_peak[..., None] - compressed) * mix[..., None]
    return np.where((peak < start_compression)[..., None], rgb, compressed)


def to_display(linear, exposure):
    return linear_to_srgb(tonemap_pbr_neutral(np.maximum(linear * exposure, 0.0))).astype(np.float32)


def srgb_to_lab(srgb):
    linear = srgb_to_linear(srgb)
    matrix = np.array([[0.4124564, 0.3575761, 0.1804375], [0.2126729, 0.7151522, 0.0721750],
                       [0.0193339, 0.1191920, 0.9503041]])
    xyz = linear @ matrix.T / np.array([0.95047, 1.0, 1.08883])
    f = np.where(xyz > 216 / 24389, np.cbrt(xyz), (24389 / 27 * xyz + 16) / 116)
    return np.stack([116 * f[..., 1] - 16, 500 * (f[..., 0] - f[..., 1]), 200 * (f[..., 1] - f[..., 2])], axis=-1)


def delta_e_2000(lab1, lab2):
    l1, a1, b1 = lab1[..., 0], lab1[..., 1], lab1[..., 2]
    l2, a2, b2 = lab2[..., 0], lab2[..., 1], lab2[..., 2]
    c_mean = (np.hypot(a1, b1) + np.hypot(a2, b2)) / 2
    g = 0.5 * (1 - np.sqrt(c_mean ** 7 / (c_mean ** 7 + 25.0 ** 7)))
    a1p, a2p = (1 + g) * a1, (1 + g) * a2
    c1, c2 = np.hypot(a1p, b1), np.hypot(a2p, b2)
    h1 = np.degrees(np.arctan2(b1, a1p)) % 360
    h2 = np.degrees(np.arctan2(b2, a2p)) % 360
    dl, dc = l2 - l1, c2 - c1
    dh = h2 - h1
    dh = np.where(dh > 180, dh - 360, np.where(dh < -180, dh + 360, dh))
    dh = np.where(c1 * c2 == 0, 0, dh)
    big_dh = 2 * np.sqrt(c1 * c2) * np.sin(np.radians(dh) / 2)
    l_mean, c_bar = (l1 + l2) / 2, (c1 + c2) / 2
    h_sum = h1 + h2
    h_bar = np.where(np.abs(h1 - h2) > 180, (h_sum + 360) / 2, h_sum / 2)
    h_bar = np.where(c1 * c2 == 0, h_sum, h_bar)
    t = (1 - 0.17 * np.cos(np.radians(h_bar - 30)) + 0.24 * np.cos(np.radians(2 * h_bar))
         + 0.32 * np.cos(np.radians(3 * h_bar + 6)) - 0.20 * np.cos(np.radians(4 * h_bar - 63)))
    d_theta = 30 * np.exp(-(((h_bar - 275) / 25) ** 2))
    r_c = 2 * np.sqrt(c_bar ** 7 / (c_bar ** 7 + 25.0 ** 7))
    s_l = 1 + 0.015 * (l_mean - 50) ** 2 / np.sqrt(20 + (l_mean - 50) ** 2)
    s_c, s_h = 1 + 0.045 * c_bar, 1 + 0.015 * c_bar * t
    r_t = -np.sin(np.radians(2 * d_theta)) * r_c
    return np.sqrt((dl / s_l) ** 2 + (dc / s_c) ** 2 + (big_dh / s_h) ** 2 + r_t * (dc / s_c) * (big_dh / s_h))


def gaussian_kernel(size=11, sigma=1.5):
    axis = np.arange(size) - size // 2
    kernel = np.exp(-(axis ** 2) / (2 * sigma ** 2))
    return kernel / kernel.sum()


def filter_valid(image, kernel):
    size = len(kernel)
    rows = np.lib.stride_tricks.sliding_window_view(image, size, axis=0) @ kernel
    return np.lib.stride_tricks.sliding_window_view(rows, size, axis=1) @ kernel


def ssim_map(a, b):
    """Gaussian-window SSIM (11x11, sigma 1.5) of two luminance images in 0..1; returns the valid-region map."""
    kernel = gaussian_kernel()
    a, b = a.astype(np.float64), b.astype(np.float64)
    mu_a, mu_b = filter_valid(a, kernel), filter_valid(b, kernel)
    var_a = filter_valid(a * a, kernel) - mu_a ** 2
    var_b = filter_valid(b * b, kernel) - mu_b ** 2
    covariance = filter_valid(a * b, kernel) - mu_a * mu_b
    c1, c2 = 0.01 ** 2, 0.03 ** 2
    return ((2 * mu_a * mu_b + c1) * (2 * covariance + c2)) / ((mu_a ** 2 + mu_b ** 2 + c1) * (var_a + var_b + c2))


def psnr(a, b):
    mse = float(np.mean((a.astype(np.float64) - b) ** 2))
    return float("inf") if mse == 0 else float(10 * np.log10(1.0 / mse))


def luminance(rgb):
    return rgb @ LUMA


def flip_scores(reference_display, test_display, reference_linear, test_linear):
    try:
        import flip_evaluator as flip
    except ImportError:
        return None
    ldr_map, ldr_mean, _ = flip.evaluate(np.ascontiguousarray(reference_display), np.ascontiguousarray(test_display), "LDR",
                                         inputsRGB=True, applyMagma=False, computeMeanError=True)
    hdr_map, hdr_mean, _ = flip.evaluate(np.ascontiguousarray(reference_linear), np.ascontiguousarray(test_linear), "HDR",
                                         inputsRGB=False, applyMagma=False, computeMeanError=True)
    return {"ldr_mean": float(ldr_mean), "ldr_p95": float(np.percentile(ldr_map, 95)),
            "hdr_mean": float(hdr_mean), "hdr_p95": float(np.percentile(hdr_map, 95))}, np.asarray(ldr_map)


# --------------------------------------------------------------------------------------------
# Masks and AOV decomposition
# --------------------------------------------------------------------------------------------

def find_layer_rgb(layers, name):
    for prefix in (f"ViewLayer.{name}", name):
        if f"{prefix}.R" in layers:
            return exr_rgb(layers, prefix)
    return None


def find_layer_scalar(layers, name):
    for key in (f"ViewLayer.{name}.X", f"ViewLayer.{name}.V", f"ViewLayer.{name}.R", f"ViewLayer.{name}.Z"):
        if key in layers:
            return layers[key]
    return None


def build_masks(layers, material_ids):
    masks = {}
    if layers is None:
        return masks
    index = find_layer_scalar(layers, "Material Index")
    if index is not None and material_ids:
        rounded = np.rint(index).astype(np.int32)
        pure = np.abs(index - rounded) < 0.02  # a pixel straddling two materials averages their ids: leave it out
        for group, pattern in DEFAULT_MATERIAL_MASKS.items():
            ids = [i for name, i in material_ids.items() if re.search(pattern, name)]
            if ids:
                masks[group] = np.isin(rounded, ids) & pure
    diffuse_direct = find_layer_rgb(layers, "Diffuse Direct")
    if diffuse_direct is not None:
        lum = luminance(diffuse_direct)
        positive = lum[lum > 0]
        if positive.size:
            threshold = np.percentile(positive, 50)
            masks["direct_lit"] = lum >= threshold
            masks["direct_shadowed"] = lum < threshold
    emission = find_layer_rgb(layers, "Emission")
    if emission is not None:
        masks["emissive"] = luminance(emission) > 0.05
    return masks


def decompose_reference(layers):
    """Luminance share of every Cycles term in the beauty image: Combined = col*(dir+ind) per lobe + emission + environment."""
    terms = {}
    for label, color, direct, indirect in (("diffuse", "Diffuse Color", "Diffuse Direct", "Diffuse Indirect"), ("glossy", "Glossy Color", "Glossy Direct", "Glossy Indirect"),
                                           ("transmission", "Transmission Color", "Transmission Direct", "Transmission Indirect")):
        col = find_layer_rgb(layers, color)
        for kind, name in (("direct", direct), ("indirect", indirect)):
            lighting = find_layer_rgb(layers, name)
            if col is not None and lighting is not None:
                terms[f"{label}_{kind}"] = col * lighting
    for label, name in (("emission", "Emission"), ("environment", "Environment")):
        value = find_layer_rgb(layers, name)
        if value is not None:
            terms[label] = value
    return terms


def direct_only_image(layers):
    terms = decompose_reference(layers)
    return sum(v for k, v in terms.items() if not k.endswith("_indirect"))


def direct_flat_ambient_image(layers):
    """Direct light plus a constant ambient: every indirect lighting pass replaced by its image-wide mean, then times the lobe colour.

    A stand-in for a renderer without GI that still lights shadows with one flat ambient (the old viewer ambient).
    """
    image = direct_only_image(layers)
    for color, indirect in (("Diffuse Color", "Diffuse Indirect"), ("Glossy Color", "Glossy Indirect"),
                            ("Transmission Color", "Transmission Indirect")):
        col, lighting = find_layer_rgb(layers, color), find_layer_rgb(layers, indirect)
        if col is not None and lighting is not None:
            image = image + col * lighting.mean(axis=(0, 1))
    return image


def mean_over(mask, values):
    return float(values[mask].mean()) if mask is not None and mask.any() else None


# --------------------------------------------------------------------------------------------
# Output
# --------------------------------------------------------------------------------------------

def heat_map(values, maximum):
    stops = np.array([[0.0, 0.0, 0.0], [0.1, 0.0, 0.4], [0.7, 0.0, 0.4], [0.95, 0.5, 0.0], [1.0, 1.0, 0.4]], dtype=np.float32)
    t = np.clip(values / maximum, 0, 1) * (len(stops) - 1)
    lower = np.minimum(t.astype(int), len(stops) - 2)
    fraction = (t - lower)[..., None]
    return stops[lower] * (1 - fraction) + stops[lower + 1] * fraction


def ratio_map(reference_lum, test_lum, span_ev=2.0):
    """Red: the test image is brighter than the reference, blue: darker; saturates at +-span_ev stops."""
    ev = np.clip(np.log2((test_lum + 1e-4) / (reference_lum + 1e-4)) / span_ev, -1, 1)
    out = np.ones(ev.shape + (3,), np.float32)
    out[..., 1] = 1 - np.abs(ev)
    out[..., 0] = np.where(ev > 0, 1.0, 1 - np.abs(ev))
    out[..., 2] = np.where(ev < 0, 1.0, 1 - np.abs(ev))
    return out


def save_png(array, path):
    Image.fromarray((np.clip(array, 0, 1) * 255 + 0.5).astype(np.uint8)).save(path)


def main():
    parser = argparse.ArgumentParser(prog="compare.py")
    parser.add_argument("--reference", required=True)
    parser.add_argument("--test")
    parser.add_argument("--synthetic", choices=["direct-only", "direct-flat-ambient"], help="use a subset of the reference's own AOV terms as the test image")
    parser.add_argument("--out-dir", required=True)
    parser.add_argument("--name", default=None)
    parser.add_argument("--test-scale", type=float, default=1.0, help="multiplies the test radiance (units calibration, one constant)")
    parser.add_argument("--exposure", type=float, default=1.0, help="applied to both images before the tonemap")
    parser.add_argument("--reference-json", default=None, help="reference.json with material_ids (default: searched above the EXR)")
    args = parser.parse_args()
    if not args.test and not args.synthetic:
        parser.error("give --test or --synthetic")

    reference_path = Path(args.reference)
    name = args.name or reference_path.stem
    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    reference, layers = load_radiance(reference_path)
    if layers is None or "ViewLayer.Material Index.X" not in layers:
        raw_sibling = reference_path.with_name(reference_path.stem.removesuffix("_denoised") + reference_path.suffix)
        if raw_sibling != reference_path and raw_sibling.exists():  # a denoised reference takes its AOVs from the raw render
            layers = read_exr_layers(raw_sibling)
    if args.synthetic:
        if layers is None:
            parser.error("--synthetic needs a multilayer EXR reference")
        build = direct_only_image if args.synthetic == "direct-only" else direct_flat_ambient_image
        test = build(layers).astype(np.float32)
    else:
        test, _ = load_radiance(args.test)
        test = test * args.test_scale
    if test.shape != reference.shape:
        parser.error(f"size mismatch: reference {reference.shape}, test {test.shape}")

    json_path = Path(args.reference_json) if args.reference_json else next(
        (p / "reference.json" for p in reference_path.resolve().parents if (p / "reference.json").exists()), None)
    material_ids = json.loads(json_path.read_text(encoding="utf-8"))["material_ids"] if json_path else {}

    reference_display, test_display = to_display(reference, args.exposure), to_display(test, args.exposure)
    reference_lum, test_lum = luminance(reference), luminance(test)
    delta_e = delta_e_2000(srgb_to_lab(reference_display), srgb_to_lab(test_display))
    ssim = ssim_map(luminance(reference_display), luminance(test_display))
    metrics = {
        "reference": str(reference_path), "test": args.test or f"synthetic:{args.synthetic}", "test_scale": args.test_scale,
        "exposure": args.exposure, "tonemap": "PBR Neutral + sRGB", "size": [reference.shape[1], reference.shape[0]],
        "psnr_db": psnr(reference_display, test_display), "ssim": float(ssim.mean()),
        "delta_e2000_mean": float(delta_e.mean()), "delta_e2000_p95": float(np.percentile(delta_e, 95)),
        "mean_luminance_reference": float(reference_lum.mean()), "mean_luminance_test": float(test_lum.mean()),
        "bias_total": float(test_lum.mean() / reference_lum.mean()),
    }
    flip_result = flip_scores(reference_display, test_display, reference, test)
    metrics["flip"] = flip_result[0] if flip_result else "flip_evaluator not installed"

    per_mask = {}
    masks = build_masks(layers, material_ids)
    for mask_name, mask in masks.items():
        ref_mean = mean_over(mask, reference_lum)
        entry = {"pixels": int(mask.sum()), "share_of_image": float(mask.mean())}
        if ref_mean:
            entry["bias_linear"] = mean_over(mask, test_lum) / ref_mean
            entry["bias_ev"] = float(np.log2(entry["bias_linear"])) if entry["bias_linear"] > 0 else None
            entry["delta_e2000_mean"] = mean_over(mask, delta_e)
        per_mask[mask_name] = entry
    metrics["per_mask"] = per_mask
    windows = masks.get("windows")
    if windows is not None and (~windows).any():  # a few bright, lit blinds carry half of the image's luminance
        metrics["bias_without_windows"] = float(test_lum[~windows].mean() / reference_lum[~windows].mean())

    if layers is not None:
        terms = decompose_reference(layers)
        total = sum(luminance(v).sum() for v in terms.values())
        metrics["reference_energy_share"] = {k: float(luminance(v).sum() / total) for k, v in terms.items()}
        indirect = sum(luminance(v).sum() for k, v in terms.items() if k.endswith("_indirect"))
        metrics["reference_indirect_share"] = float(indirect / total)

    (out_dir / f"{name}_metrics.json").write_text(json.dumps(metrics, indent=2), encoding="utf-8")
    save_png(heat_map(delta_e, 20.0), out_dir / f"{name}_delta_e.png")
    collage = np.concatenate([reference_display, test_display, heat_map(delta_e, 20.0), ratio_map(reference_lum, test_lum)], axis=1)
    save_png(collage, out_dir / f"{name}_collage.png")
    print(json.dumps({k: v for k, v in metrics.items() if k not in ("per_mask",)}, indent=2))
    print("per_mask:", json.dumps(per_mask, indent=1))


if __name__ == "__main__":
    sys.exit(main())
