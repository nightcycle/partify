#!/usr/bin/env bash
# Run from any directory with Bash (including Git Bash on Windows).
set -Eeuo pipefail

usage() {
    cat <<'EOF'
Usage: bash scripts/test.sh [OPTIONS] [INPUT ...]

Run code checks, packaging tests, reference conversions, and your own inputs.
INPUT can be any PNG/JPG/JPEG/OBJ/FBX/RBXM/RBXMX file or folder to scan recursively.
With no inputs, scans examples/inputs. Quote filenames containing spaces.
Outputs are saved under target/test-run/; input files are never regenerated.

  --input PATH       Select a file or folder to test (repeatable).
  --offline          Disable Cargo and Roblox downloads.
  --window           Preview the first successful input, even if others fail.
  --budget N         Part budget per user input (default: 24).
  --samples N        Sample limit per conversion (default: 1024).
  --min-coverage N   Optional coverage threshold for user inputs, 0..1 (default: 0).
  --asset-map FILE   JSON map of Roblox asset URIs to local files.
  --asset-cache DIR  Roblox mesh/texture cache directory.
  --help             Show this help.
  --                 Treat remaining arguments as input paths.

Paths are relative to your current directory. Requires Rust with rustfmt/clippy,
a C/C++ toolchain, Python 3.11+, and the native window dependencies in README.md.
Set PYTHON to override the Python executable.

Preview a specific input:
  bash scripts/test.sh --input examples/inputs/tree.rbxm --window
  bash scripts/test.sh --offline --window "examples/inputs/fish.jpg"
EOF
}

cargo_options=(--locked)
input_options=()
while (($#)); do
    case "$1" in
        --offline) cargo_options+=(--offline); input_options+=("$1"); shift ;;
        --window) input_options+=("$1"); shift ;;
        --input|--budget|--samples|--min-coverage|--asset-map|--asset-cache)
            if (($# < 2)); then printf 'Missing value for %s\n' "$1" >&2; exit 2; fi
            input_options+=("$1" "$2"); shift 2 ;;
        --help|-h) usage; exit 0 ;;
        --) input_options+=("$@"); break ;;
        -*) printf 'Unknown option: %s\n' "$1" >&2; usage >&2; exit 2 ;;
        *) input_options+=("$1"); shift ;;
    esac
done

caller_dir="$(pwd)"
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd -- "$repo_root"
trap 'printf "\nTest run failed at line %s.\n" "$LINENO" >&2' ERR

command -v cargo >/dev/null || { printf 'cargo is required.\n' >&2; exit 1; }
python_bin="${PYTHON:-}"
if [[ -z "$python_bin" ]]; then
    for candidate in python3 python; do
        if command -v "$candidate" >/dev/null && "$candidate" -c 'import sys; sys.exit(sys.version_info < (3, 11))' 2>/dev/null; then
            python_bin="$candidate"
            break
        fi
    done
fi
if [[ -z "$python_bin" ]] || ! "$python_bin" -c 'import sys; sys.exit(sys.version_info < (3, 11))'; then
    printf 'Python 3.11+ is required; set PYTHON to its executable.\n' >&2
    exit 1
fi

# Reject missing/unsupported paths before spending time compiling and testing.
"$python_bin" scripts/smoke_inputs.py --base-dir "$caller_dir" --check-inputs "${input_options[@]}"

printf '\nChecking formatting and Clippy…\n'
cargo fmt --all -- --check
cargo clippy "${cargo_options[@]}" --all-targets --all-features -- -D warnings

printf '\nRunning Rust tests and checking the headless build…\n'
cargo test "${cargo_options[@]}" --all-features
cargo check "${cargo_options[@]}" --no-default-features

printf '\nTesting release packaging and input handling…\n'
"$python_bin" -m unittest discover -s scripts -p 'test_*.py'

"$python_bin" scripts/smoke_inputs.py --base-dir "$caller_dir" "${input_options[@]}"
