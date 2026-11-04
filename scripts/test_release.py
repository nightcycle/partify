import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

SPEC = importlib.util.spec_from_file_location("release", Path(__file__).with_name("release.py"))
release = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release)


class ReleaseTests(unittest.TestCase):
    def test_archives_have_rokit_names_executable_and_permissions(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Cargo.toml").write_text('[package]\nname="partify"\nversion="0.1.0"\n')
            (root / "Cargo.lock").write_text('[[package]]\nname="partify"\nversion="0.1.0"\n')
            for name in ["LICENSE", "README.md"]:
                (root / name).write_text(name)
            with patch.object(release, "ROOT", root), patch.object(release, "validate"):
                for platform, target in release.TARGETS.items():
                    exe = "partify.exe" if platform.startswith("windows") else "partify"
                    binary = root / "target" / target / "release" / exe
                    binary.parent.mkdir(parents=True)
                    binary.write_bytes(b"executable fixture")
                    release.package("v0.1.0", platform)
                    archive = root / "dist" / f"partify-v0.1.0-{platform}.zip"
                    with zipfile.ZipFile(archive) as z:
                        self.assertEqual(z.read(exe), b"executable fixture")
                        self.assertEqual(z.getinfo(exe).external_attr >> 16, 0o100755)
                release.checksums(root / "dist")
                self.assertEqual(len((root / "dist" / "SHA256SUMS").read_text().splitlines()), 5)
            release.validate("v0.1.0", root)
            with self.assertRaises(SystemExit):
                release.validate("v0.2.0", root)


if __name__ == "__main__":
    unittest.main()
