import contextlib
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import smoke_inputs


def report(path, coverage=0.5):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({
        "parts": [{}], "tuning": {"budget": 24},
        "metrics": {"parts": 1, "coverage": coverage, "sampled_overlap": 0},
    }), encoding="utf-8")
    path.with_suffix(".rbxm").write_bytes(b"model")


class InputTests(unittest.TestCase):
    def test_default_folder_discovers_renamed_nested_inputs(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            inputs = root / "examples" / "inputs"
            (inputs / "sub folder").mkdir(parents=True)
            files = [inputs / "renamed image.PNG", inputs / "sub folder" / "fish.JpEg"]
            for file in files:
                file.write_bytes(b"input")
            (inputs / "materials.mtl").write_text("material")
            with patch.object(smoke_inputs, "ROOT", root):
                self.assertEqual(smoke_inputs.discover_inputs([], root), files)
            self.assertEqual(smoke_inputs.discover_inputs([files[0], inputs], root), files)
            self.assertEqual(smoke_inputs.discover_inputs([Path("inputs/renamed image.PNG")], inputs.parent), [files[0]])

    def test_bad_paths_fail_with_the_actual_path(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            (root / "empty").mkdir()
            (root / "notes.txt").write_text("notes")
            for value in ["missing.png", "notes.txt", "empty"]:
                with self.subTest(value=value), self.assertRaisesRegex(ValueError, value):
                    smoke_inputs.discover_inputs([Path(value)], root)

    def test_low_coverage_is_optional_but_budget_and_overlap_remain_checked(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "report.json"
            report(path, 0.4)
            self.assertIn("40.0%", smoke_inputs.verify_report(path, 0))
            with self.assertRaisesRegex(ValueError, "below requested"):
                smoke_inputs.verify_report(path, 0.9)
            data = json.loads(path.read_text())
            data["tuning"]["budget"] = 0
            path.write_text(json.dumps(data))
            with self.assertRaisesRegex(ValueError, "part count"):
                smoke_inputs.verify_report(path, 0)

    def test_mixed_arguments_spaces_unique_outputs_and_continuing_after_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            inputs = [root / "same name.PNG", root / "same name.jpg", root / "later.obj"]
            for source in inputs:
                source.write_bytes(b"original input")
            calls = []

            def run(command, **kwargs):
                calls.append(command)
                source = Path(command[command.index("--") + 1])
                output = Path(command[command.index("--report") + 1])
                if source == inputs[1]:
                    # A stale successful report must not hide a failed conversion.
                    report(output, 1)
                    raise subprocess.CalledProcessError(1, command)
                report(output, 0.5 if source in inputs else 1)
                return subprocess.CompletedProcess(command, 0)

            stderr = io.StringIO()
            with patch.object(smoke_inputs, "ROOT", root), patch.object(smoke_inputs.subprocess, "run", side_effect=run), contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(stderr):
                status = smoke_inputs.main([str(inputs[0]), "--budget", "24", str(inputs[1]), str(inputs[2])])
            self.assertEqual(status, 1)
            self.assertIn(str(inputs[1]), stderr.getvalue())
            self.assertEqual(len(calls), 7)  # Four references, three user files.
            outputs = [command[command.index("--output") + 1] for command in calls]
            self.assertEqual(len(set(outputs)), len(outputs))
            self.assertEqual(Path(calls[-1][calls[-1].index("--") + 1]), inputs[-1])
            self.assertNotIn("--offline", calls[-1])
            for source in inputs:
                self.assertEqual(source.read_bytes(), b"original input")

    def test_offline_asset_options_forwarded(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            source = root / "model.rbxm"
            source.write_bytes(b"model")
            mapping = root / "asset map.json"
            mapping.write_text("{}")
            calls = []

            def run(command, **kwargs):
                calls.append(command)
                report(Path(command[command.index("--report") + 1]), 1)
                return subprocess.CompletedProcess(command, 0)

            with patch.object(smoke_inputs, "ROOT", root), patch.object(smoke_inputs.subprocess, "run", side_effect=run), contextlib.redirect_stdout(io.StringIO()):
                status = smoke_inputs.main(["--base-dir", str(root), "--offline", "--asset-map", "asset map.json", "--asset-cache", "cache", "model.rbxm"])
            self.assertEqual(status, 0)
            custom = calls[-1]
            self.assertEqual(custom.count("--offline"), 2)
            self.assertEqual(custom[custom.index("--asset-map") + 1], str(mapping))
            self.assertEqual(custom[custom.index("--asset-cache") + 1], str(root / "cache"))

    def test_window_previews_successful_inputs_without_hiding_failures(self):
        scenarios = [
            ("success", set(), 0, "first.rbxm", 0),
            ("first input fails", {"first.rbxm"}, 0, "second.png", 1),
            ("all inputs fail", {"first.rbxm", "second.png"}, 0, None, 1),
            ("reference fails", {"blender_279_default_6100_ascii.fbx"}, 0, "first.rbxm", 1),
            ("preview fails", set(), 2, "first.rbxm", 1),
            ("preview cannot launch", set(), OSError("cannot launch"), "first.rbxm", 1),
        ]
        for name, failed_sources, window_status, preview_name, expected_status in scenarios:
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                root = Path(directory).resolve()
                inputs = [root / "first.rbxm", root / "second.png"]
                for source in inputs:
                    source.write_bytes(b"input")
                previews = []

                def run(command, **kwargs):
                    source = Path(command[command.index("--") + 1])
                    if "--window" in command:
                        previews.append(source)
                        self.assertEqual(command.count("--offline"), 2)
                        if isinstance(window_status, OSError):
                            raise window_status
                        return subprocess.CompletedProcess(command, window_status)
                    if source.name in failed_sources:
                        raise subprocess.CalledProcessError(1, command)
                    report(Path(command[command.index("--report") + 1]), 1)
                    return subprocess.CompletedProcess(command, 0)

                stdout = io.StringIO()
                stderr = io.StringIO()
                with patch.object(smoke_inputs, "ROOT", root), patch.object(smoke_inputs.subprocess, "run", side_effect=run), contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                    status = smoke_inputs.main(["--offline", "--window", *map(str, inputs)])
                self.assertEqual(status, expected_status)
                self.assertEqual(previews, [root / preview_name] if preview_name else [])
                self.assertEqual("All tests passed." in stdout.getvalue(), status == 0)
                if preview_name is None:
                    self.assertIn("no preview to open", stderr.getvalue())

    def test_selected_input_controls_conversions_and_preview(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            selected = root / "models" / "my tree.rbxm"
            positional = root / "fish.jpg"
            default_input = root / "examples" / "inputs" / "default.png"
            for source in [selected, positional, default_input]:
                source.parent.mkdir(parents=True, exist_ok=True)
                source.write_bytes(b"input")
            scenarios = [
                (["--input", "models/my tree.rbxm"], [selected]),
                (["fish.jpg", "--input", "models/my tree.rbxm", "--input", "models/my tree.rbxm"],
                 [selected, positional]),
            ]
            for arguments, expected_inputs in scenarios:
                with self.subTest(arguments=arguments):
                    conversions = []
                    previews = []

                    def run(command, **kwargs):
                        source = Path(command[command.index("--") + 1])
                        if "--window" in command:
                            previews.append(source)
                        else:
                            conversions.append(source)
                            report(Path(command[command.index("--report") + 1]), 1)
                        return subprocess.CompletedProcess(command, 0)

                    with patch.object(smoke_inputs, "ROOT", root), patch.object(smoke_inputs.subprocess, "run", side_effect=run), contextlib.redirect_stdout(io.StringIO()):
                        status = smoke_inputs.main(["--base-dir", str(root), "--offline", "--window", *arguments])
                    self.assertEqual(status, 0)
                    self.assertEqual(conversions[4:], expected_inputs)
                    self.assertEqual(previews, [selected])
                    self.assertNotIn(default_input, conversions)


if __name__ == "__main__":
    unittest.main()
