"""Maintenance-only CPU ComfyUI schema capture and workerless graph validation."""

import argparse
import asyncio
import copy
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
import tempfile

COMFYUI_COMMIT = "f1072eb0350638a3390ddb6afbcaa8c6b237c6fd"
FOLDERS = {
    "checkpoints": "checkpoint", "diffusion_models": "diffusion", "unet": "diffusion",
    "text_encoders": "text-encoder", "clip": "text-encoder", "vae": "vae", "loras": "lora",
}


def read_json(path: Path):
    with path.open("rb") as stream:
        data = stream.read((64 << 20) + 1)
    if len(data) > 64 << 20:
        raise ValueError(f"{path}: JSON exceeds 64 MiB")
    return json.loads(data)


def select_candidates(registry: dict, report: dict) -> list[dict]:
    if report.get("complete") is not True or report.get("failures") != []:
        raise ValueError("incomplete harvest: refusing a candidate proposal")
    selected = report.get("candidates")
    if not isinstance(selected, dict) or len(selected) > 10:
        raise ValueError("expected at most ten reported candidates")
    if registry.get("schemaVersion") != 1:
        raise ValueError("unsupported workflow registry schema")
    records = registry["workflows"]
    by_id = {item["id"]: item for item in records}
    if len(by_id) != len(records):
        raise ValueError("duplicate workflow id")
    candidates = []
    for identifier in selected:
        item = by_id.get(identifier)
        if item is None or item.get("status") != "candidate":
            raise ValueError(f"{identifier}: reported candidate missing or not a candidate")
        candidates.append(item)
    return candidates


def check_changes(before: dict, after: dict, selected: set[str]) -> None:
    if before.keys() != after.keys():
        raise ValueError("harvest changed the registry structure")
    if before.get("schemaVersion") != after.get("schemaVersion"):
        raise ValueError("harvest changed the workflow registry schema version")
    old = {item["id"]: item for item in before["workflows"]}
    new = {item["id"]: item for item in after["workflows"]}
    if old.keys() - new.keys():
        raise ValueError("harvest removed an existing workflow")
    for identifier in old.keys() | new.keys():
        if old.get(identifier) != new.get(identifier):
            if identifier not in selected or new[identifier]["status"] != "candidate" \
                    or old.get(identifier, {}).get("status") == "verified":
                raise ValueError(f"harvest changed an unreported or verified workflow: {identifier}")


def prepare_graph(item: dict, base: Path) -> dict:
    if item.get("status") != "candidate" or "cpu" not in item.get("deviceClasses", []):
        raise ValueError("only CPU-schema-compatible candidates may be validated here")
    graph = copy.deepcopy(item["graph"])
    if not isinstance(graph, dict) or not 1 <= len(graph) <= 512:
        raise ValueError("graph must contain 1..512 nodes")
    if len(json.dumps(graph).encode()) > 4 << 20:
        raise ValueError("graph exceeds 4 MiB")
    if not 1 <= len(item["models"]) <= 32:
        raise ValueError("expected 1..32 pinned model slots")
    for slot in item["models"]:
        weight = slot["weight"]
        name = weight["file"]
        folder = weight["folder"]
        if (FOLDERS.get(folder) != slot["role"] or folder not in FOLDERS
                or not isinstance(name, str) or not name
                or "\\" in name or "\0" in name
                or PurePosixPath(name).is_absolute()
                or any(part in ("", ".", "..") for part in name.split("/"))
                or not name.endswith(".safetensors")):
            raise ValueError("unsafe model placeholder path or role")
        if (not re.fullmatch(r"[0-9a-f]{40}", weight["revision"])
                or not re.fullmatch(r"[0-9a-f]{64}", weight["sha256"])
                or type(weight["bytes"]) is not int or weight["bytes"] <= 0):
            raise ValueError("model slot lacks an exact weight pin")
        placeholder = base / "models" / folder / name
        placeholder.parent.mkdir(parents=True, exist_ok=True)
        placeholder.touch(exist_ok=True)
        if placeholder.stat().st_size != 0:
            raise ValueError("validation directory contains real weights")
        graph[slot["node"]]["inputs"][slot["input"]] = name
    return graph


def forbid_execution(*_args, **_kwargs):
    raise RuntimeError("workflow validation must never enqueue or execute a graph")


async def validate_graph(item: dict, graph: dict, validate) -> dict:
    valid, error, outputs, node_errors = await validate(item["id"], graph, None)
    output = item["bindings"]["output"]["node"]
    return {
        "id": item["id"],
        "valid": valid is True and error is None and not node_errors and output in outputs,
        "error": error, "nodeErrors": node_errors, "outputs": outputs,
    }


async def run_comfy(args, base: Path, candidates: list[dict]) -> None:
    source = args.comfyui.resolve()
    actual = subprocess.check_output(
        ["git", "-C", str(source), "rev-parse", "HEAD"], text=True).strip()
    if actual != COMFYUI_COMMIT:
        raise ValueError(f"ComfyUI must be pinned to {COMFYUI_COMMIT}, got {actual}")
    subprocess.run(["git", "-C", str(source), "diff", "--exit-code", "HEAD", "--"],
                   check=True, stdout=subprocess.DEVNULL)
    if (source / "extra_model_paths.yaml").exists():
        raise ValueError("external model path configuration is forbidden for validation")
    graphs = [(item, prepare_graph(item, base)) for item in candidates]
    os.environ["HF_HUB_OFFLINE"] = "1"
    os.environ["HF_HUB_DISABLE_TELEMETRY"] = "1"
    os.environ["DO_NOT_TRACK"] = "1"
    sys.path.insert(0, str(source))
    sys.argv = [
        str(source / "main.py"), "--cpu", "--base-directory", str(base),
        "--disable-all-custom-nodes", "--disable-api-nodes", "--disable-auto-launch",
    ]
    import main
    import execution
    import nodes

    # Never call start_comfyui: it creates a prompt worker. Guard both execution
    # boundaries as well, so future changes cannot accidentally queue a graph.
    execution.PromptQueue.put = forbid_execution
    execution.PromptExecutor.execute = forbid_execution
    manager = main.default_asset_manager()
    server = main.server.PromptServer(asyncio.get_running_loop(), manager)
    try:
        await nodes.init_extra_nodes(init_custom_nodes=False, init_api_nodes=False)
        if args.command == "capture":
            route = next(route for route in server.routes
                         if route.method == "GET" and route.path == "/object_info")
            response = await route.handler(None)
            schema = json.loads(response.body)
            if not schema or "KSampler" not in schema:
                raise ValueError("incomplete CPU ObjectInfo")
            args.output.write_text(json.dumps(schema, indent=2) + "\n")
        else:
            results = []
            for item, graph in graphs:
                for graph_node in graph.values():
                    cls = nodes.NODE_CLASS_MAPPINGS[graph_node["class_type"]]
                    module = getattr(cls, "RELATIVE_PYTHON_MODULE", "nodes")
                    if (module != "nodes" and not module.startswith("comfy_extras.")) \
                            or getattr(cls, "API_NODE", False):
                        raise ValueError("only native, local ComfyUI nodes may validate")
                results.append(await asyncio.wait_for(
                    validate_graph(item, graph, execution.validate_prompt), timeout=60))
            report = {
                "comfyuiCommit": COMFYUI_COMMIT, "deviceClass": "cpu",
                "mode": "validation-only-no-worker", "executedGraphs": 0,
                "valid": all(result["valid"] for result in results), "results": results,
            }
            args.output.write_text(json.dumps(report, indent=2) + "\n")
            if not report["valid"]:
                raise ValueError("CPU graph validation failed; see validation report")
    finally:
        manager.shutdown()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("capture", "validate"))
    parser.add_argument("--comfyui", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--workflows", type=Path)
    parser.add_argument("--harvest-report", type=Path)
    parser.add_argument("--baseline", type=Path)
    args = parser.parse_args()
    candidates = []
    if args.command == "validate":
        if args.workflows is None or args.harvest_report is None:
            parser.error("validate requires --workflows and --harvest-report")
        registry = read_json(args.workflows)
        candidates = select_candidates(registry, read_json(args.harvest_report))
        if args.baseline is not None:
            check_changes(read_json(args.baseline), registry, {item["id"] for item in candidates})
    with tempfile.TemporaryDirectory(prefix="rigspark-workflow-validation-") as directory:
        asyncio.run(run_comfy(args, Path(directory), candidates))


if __name__ == "__main__":
    main()
