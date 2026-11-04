"""CLI smoke tests for arbitrary user files, plus stable offline reference cases."""
import argparse
import json
import math
from pathlib import Path
import re
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
EXTENSIONS = {".png", ".jpg", ".jpeg", ".obj", ".fbx", ".rbxm", ".rbxmx"}


def discover_inputs(paths, base_dir):
    """Resolve caller-relative files/directories; preserve order and deduplicate."""
    found = []
    seen = set()
    for value in paths or [ROOT / "examples" / "inputs"]:
        path = (base_dir / value).resolve()
        if not path.exists():
            raise ValueError(f"Input does not exist: {path}")
        if path.is_dir():
            matches = sorted(
                (p for p in path.rglob("*") if p.is_file() and p.suffix.lower() in EXTENSIONS),
                key=lambda p: p.as_posix().casefold(),
            )
            if not matches:
                raise ValueError(f"No supported input files in: {path}")
        elif path.is_file() and path.suffix.lower() in EXTENSIONS:
            matches = [path]
        else:
            raise ValueError(f"Unsupported input: {path}; expected {', '.join(sorted(EXTENSIONS))}")
        for match in matches:
            resolved = match.resolve()
            if resolved not in seen:
                seen.add(resolved)
                found.append(resolved)
    return found


def verify_report(path, min_coverage):
    report = json.loads(path.read_text(encoding="utf-8"))
    metrics = report["metrics"]
    count = len(report["parts"])
    budget = report["tuning"]["budget"]
    if not 0 < count <= budget or metrics["parts"] != count:
        raise ValueError(f"invalid part count {count}/{budget}")
    if not all(math.isfinite(value) for value in metrics.values()):
        raise ValueError("non-finite metrics")
    if not 0 <= metrics["coverage"] <= 1 or not 0 <= metrics["sampled_overlap"] <= 1:
        raise ValueError("invalid coverage/overlap range")
    if metrics["coverage"] < min_coverage:
        raise ValueError(f"coverage {metrics['coverage']:.1%} is below requested {min_coverage:.1%}")
    if metrics["sampled_overlap"] > 0.001:
        raise ValueError(f"overlapping interiors: {metrics['sampled_overlap']:.2%}")
    if not path.with_suffix(".rbxm").is_file():
        raise ValueError("missing output model")
    return (f"{count}/{budget} parts, {metrics['coverage']:.1%} coverage, "
            f"{metrics['sampled_overlap']:.2%} overlap")


def parser():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("inputs", nargs="*", type=Path, help="Files or folders; defaults to examples/inputs")
    p.add_argument("--input", action="append", type=Path, default=[], dest="selected_inputs",
                   help="Select a file or folder to test (repeatable; takes preview priority over positional inputs)")
    p.add_argument("--base-dir", type=Path, default=Path.cwd(), help=argparse.SUPPRESS)
    p.add_argument("--check-inputs", action="store_true", help=argparse.SUPPRESS)
    p.add_argument("--offline", action="store_true", help="Disable Cargo and Roblox downloads")
    p.add_argument("--window", action="store_true", help="Preview the first successful input, even if others fail")
    p.add_argument("--budget", type=int, default=24, help="Part budget for user inputs (default: 24)")
    p.add_argument("--samples", type=int, default=1024, help="Sample limit (default: 1024)")
    p.add_argument("--min-coverage", type=float, default=0, help="Optional user-input coverage threshold, 0..1")
    p.add_argument("--asset-map", type=Path, help="Map Roblox asset URIs to local files")
    p.add_argument("--asset-cache", type=Path, help="Roblox asset cache directory")
    return p


def main(argv=None):
    p = parser()
    args = p.parse_intermixed_args(argv)
    if not 1 <= args.budget <= 10000:
        p.error("--budget must be 1..10000")
    if not 128 <= args.samples <= 250000:
        p.error("--samples must be 128..250000")
    if not 0 <= args.min_coverage <= 1:
        p.error("--min-coverage must be 0..1")
    try:
        inputs = discover_inputs(args.selected_inputs + args.inputs, args.base_dir)
        if args.asset_map:
            args.asset_map = (args.base_dir / args.asset_map).resolve()
            if not args.asset_map.is_file():
                raise ValueError(f"Asset map does not exist: {args.asset_map}")
        if args.asset_cache:
            args.asset_cache = (args.base_dir / args.asset_cache).resolve()
    except ValueError as error:
        p.error(str(error))
    if args.check_inputs:
        print(f"Found {len(inputs)} supported input(s):", flush=True)
        for path in inputs:
            print(f"  {path}", flush=True)
        return 0

    output_root = ROOT / "target" / "test-run"
    output_root.mkdir(parents=True, exist_ok=True)
    # A fresh run folder also protects inputs taken from previous test outputs.
    out = Path(tempfile.mkdtemp(prefix="run-", dir=output_root))
    cargo = ["cargo", "run", "--locked"]
    if args.offline:
        cargo.append("--offline")
    cargo += ["--release", "--bin", "partify", "--"]
    asset_options = ["--offline"] if args.offline else []
    for option, value in [("--asset-map", args.asset_map), ("--asset-cache", args.asset_cache)]:
        if value:
            asset_options += [option, str(value)]

    failures = []
    summaries = []
    successful_inputs = []

    def convert(name, source, budget, min_coverage, options):
        prefix = out / name
        model = prefix.with_suffix(".rbxm")
        report = prefix.with_suffix(".json")
        command = cargo + [str(source), "--budget", str(budget), "--samples", str(args.samples),
                           "--output", str(model), "--report", str(report),
                           "--save-tuning", str(prefix.with_suffix(".tuning.json"))] + options
        print(f"\nConverting {source}...", flush=True)
        try:
            subprocess.run(command, cwd=ROOT, check=True)
            summary = verify_report(report, min_coverage)
        except (subprocess.CalledProcessError, OSError, ValueError, KeyError, TypeError) as error:
            # Never validate stale output from a previous successful run.
            failures.append(str(source))
            print(f"FAILED: {source}: {error}", file=sys.stderr, flush=True)
            return False
        summaries.append(f"{source.name}: {summary} -> {model}")
        return True

    # Keep strict quality assertions for owned fixtures. A user input may
    # intentionally have a budget too small for high coverage.
    convert("reference-fbx-ascii", ROOT / "tests/fixtures/blender_279_default_6100_ascii.fbx", 12, 0.9, ["--offline"])
    convert("reference-fbx-binary", ROOT / "tests/fixtures/blender_272_cube_7400_binary.fbx", 12, 0.9, ["--offline"])
    if convert("reference-rbxmx", ROOT / "assets/corner-triangle.rbxmx", 1, 0.9, ["--offline"]):
        model = out / "reference-rbxmx.rbxm"
        convert("reference-rbxm", model, 1, 0.9, ["--offline", "--tuning-from", str(model)])
    for index, source in enumerate(inputs, 1):
        # Extension + index disambiguate equal stems and different folders.
        safe_name = re.sub(r"[^A-Za-z0-9_-]", "-", source.name)[:100]
        if convert(f"input-{index:03d}-{safe_name}", source, args.budget, args.min_coverage, asset_options):
            successful_inputs.append(source)

    print("\nConversion results:", flush=True)
    for summary in summaries:
        print(f"  {summary}", flush=True)
    if failures:
        print(f"\n{len(failures)} conversion(s) failed:", file=sys.stderr)
        for source in failures:
            print(f"  {source}", file=sys.stderr)
        print("See the per-file errors above. Missing Roblox mesh/texture assets require "
              "a populated --asset-cache or --asset-map; omit --offline to try public downloads.",
              file=sys.stderr)
    status = 1 if failures else 0
    print(f"\nOutputs: {out}", flush=True)
    if args.window:
        if successful_inputs:
            source = successful_inputs[0]
            print(f"\nOpening preview for {source}...", flush=True)
            command = cargo + [str(source), "--budget", str(args.budget), "--samples", str(args.samples),
                               "--window", "--output", str(out / "interactive.rbxm")] + asset_options
            try:
                window_status = subprocess.run(command, cwd=ROOT).returncode
            except OSError as error:
                print(f"FAILED to open preview: {error}", file=sys.stderr)
                window_status = 1
            if window_status:
                print("Preview failed; see the error above.", file=sys.stderr)
                status = 1
        else:
            print("No user inputs converted successfully; no preview to open.", file=sys.stderr)
    if status == 0:
        print("\nAll tests passed.", flush=True)
    return status


if __name__ == "__main__":
    sys.exit(main())
