import asyncio
import copy
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import workflow_validate as validator


def candidate():
    return {
        "id": "test-candidate", "status": "candidate", "deviceClasses": ["cpu"],
        "graph": {"1": {"class_type": "Loader", "inputs": {"name": "model.safetensors"}},
                  "2": {"class_type": "SaveImage", "inputs": {"images": ["1", 0]}}},
        "models": [{"node": "1", "input": "name", "role": "checkpoint", "weight": {
            "folder": "checkpoints", "file": "nested/model.safetensors",
            "revision": "a" * 40, "sha256": "b" * 64, "bytes": 123,
        }}],
        "bindings": {"output": {"node": "2", "format": "png"}},
    }


class ValidationTests(unittest.TestCase):
    def test_incomplete_or_mismatched_reports_cannot_propose(self):
        registry = {"schemaVersion": 1, "workflows": [candidate()]}
        good = {"complete": True, "failures": [], "candidates": {"test-candidate": {}}}
        self.assertEqual(len(validator.select_candidates(registry, good)), 1)
        for report in [
            dict(good, complete=False), dict(good, complete="true"),
            dict(good, failures=["network failure"]),
            dict(good, candidates={"missing": {}}),
        ]:
            with self.subTest(report=report), self.assertRaises(ValueError):
                validator.select_candidates(registry, report)
        registry["workflows"][0]["status"] = "verified"
        with self.assertRaises(ValueError):
            validator.select_candidates(registry, good)

    def test_placeholders_are_empty_isolated_and_bind_exact_nested_filename(self):
        original = candidate()
        before = copy.deepcopy(original)
        with tempfile.TemporaryDirectory() as tmp:
            graph = validator.prepare_graph(original, Path(tmp))
            self.assertEqual(graph["1"]["inputs"]["name"], "nested/model.safetensors")
            self.assertEqual(
                (Path(tmp) / "models/checkpoints/nested/model.safetensors").read_bytes(), b"")
        self.assertEqual(original, before)

    def test_rejects_unsafe_or_unpinned_files_and_non_cpu_candidates(self):
        for field, value in [
            ("file", "../escape.safetensors"), ("file", "/tmp/escape.safetensors"),
            ("file", "x\\escape.safetensors"), ("folder", "../outside"),
            ("sha256", "unverified"), ("revision", "main"), ("bytes", 0),
        ]:
            item = candidate()
            item["models"][0]["weight"][field] = value
            with tempfile.TemporaryDirectory() as tmp, self.subTest(field=field, value=value):
                with self.assertRaises(ValueError):
                    validator.prepare_graph(item, Path(tmp))
        item = candidate()
        item["deviceClasses"] = ["cuda"]
        with tempfile.TemporaryDirectory() as tmp, self.assertRaises(ValueError):
            validator.prepare_graph(item, Path(tmp))

    def test_partial_output_validation_is_failure(self):
        async def validate(_id, _graph, _targets):
            return True, None, ["2"], {"1": {"errors": ["invalid"]}}
        result = asyncio.run(validator.validate_graph(candidate(), candidate()["graph"], validate))
        self.assertFalse(result["valid"])
        self.assertTrue(result["nodeErrors"])

    def test_all_output_nodes_must_validate(self):
        async def validate(_id, _graph, _targets):
            return True, None, ["different-output"], {}
        result = asyncio.run(validator.validate_graph(candidate(), candidate()["graph"], validate))
        self.assertFalse(result["valid"])

    def test_validation_does_not_execute_or_enqueue(self):
        calls = []

        async def validate(_id, graph, targets):
            calls.append((graph, targets))
            return True, None, ["2"], {}
        result = asyncio.run(validator.validate_graph(candidate(), candidate()["graph"], validate))
        self.assertTrue(result["valid"])
        self.assertEqual(calls, [(candidate()["graph"], None)])
        with self.assertRaises(RuntimeError):
            validator.forbid_execution()

    def test_proposals_cannot_change_or_remove_unreported_workflows(self):
        original = {"schemaVersion": 1, "workflows": [
            dict(candidate(), id="shipped", status="verified")]}
        current = copy.deepcopy(original)
        current["workflows"].append(candidate())
        validator.check_changes(original, current, {"test-candidate"})
        current["workflows"][0]["graph"]["1"]["inputs"]["name"] = "changed"
        with self.assertRaises(ValueError):
            validator.check_changes(original, current, {"test-candidate"})
        current["workflows"] = [candidate()]
        with self.assertRaises(ValueError):
            validator.check_changes(original, current, {"test-candidate"})
        current = copy.deepcopy(original)
        current["schemaVersion"] = 2
        with self.assertRaises(ValueError):
            validator.check_changes(original, current, set())


if __name__ == "__main__":
    unittest.main()
