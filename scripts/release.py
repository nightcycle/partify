"""Validate versions and create deterministic, Rokit-compatible executable ZIPs."""
import hashlib
from pathlib import Path
import re
import sys
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parent.parent
TARGETS = {
    "windows-x86_64": "x86_64-pc-windows-msvc",
    "linux-x86_64": "x86_64-unknown-linux-gnu",
    "linux-aarch64": "aarch64-unknown-linux-gnu",
    "macos-x86_64": "x86_64-apple-darwin",
    "macos-aarch64": "aarch64-apple-darwin",
}


def validate(tag, root=ROOT):
    root = Path(root)
    version = tomllib.loads((root / "Cargo.toml").read_text())["package"]["version"]
    if not re.fullmatch(r"v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?", tag) or tag != f"v{version}":
        raise SystemExit(f"Tag {tag!r} must match Cargo.toml v{version}")
    lock = tomllib.loads((root / "Cargo.lock").read_text())
    if [p["version"] for p in lock["package"] if p["name"] == "partify"] != [version]:
        raise SystemExit("Cargo.lock package version does not match Cargo.toml")
    print(f"Validated {tag}")


def package(tag, platform, binary_path=None):
    validate(tag)
    exe = "partify.exe" if platform.startswith("windows") else "partify"
    binary = Path(binary_path) if binary_path else ROOT / "target" / TARGETS[platform] / "release" / exe
    out = ROOT / "dist"
    out.mkdir(exist_ok=True)
    archive = out / f"partify-{tag}-{platform}.zip"
    # Rokit discovers the platform from the filename and the executable at ZIP root.
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as z:
        for path, name in [(binary, exe), (ROOT / "LICENSE", "LICENSE"), (ROOT / "README.md", "README.md")]:
            info = zipfile.ZipInfo(name, (2026, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.create_system = 3
            info.external_attr = (0o100755 if name == exe else 0o100644) << 16
            z.writestr(info, path.read_bytes())
    print(archive)


def checksums(directory):
    directory = Path(directory)
    files = sorted(directory.glob("*.zip"))
    if len(files) != len(TARGETS):
        raise SystemExit(f"Expected {len(TARGETS)} platform archives, found {len(files)}")
    (directory / "SHA256SUMS").write_text("".join(
        f"{hashlib.sha256(p.read_bytes()).hexdigest()}  {p.name}\n" for p in files
    ), encoding="utf-8")


if __name__ == "__main__":
    command, *args = sys.argv[1:]
    {"validate": validate, "package": package, "checksums": checksums}[command](*args)
