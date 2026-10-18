use std::{fs, path::PathBuf};

#[test]
fn harvest_ci_is_trusted_bounded_workerless_and_candidate_only() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let workflow = fs::read_to_string(root.join(".github/workflows/workflow-harvest.yml")).unwrap();
    for required in [
        "schedule:",
        "workflow_dispatch:",
        "github.ref == 'refs/heads/main'",
        "timeout-minutes: 40",
        "cancel-in-progress: false",
        "test \"$(git rev-parse HEAD)\" = \"$GITHUB_SHA\"",
        "f1072eb0350638a3390ddb6afbcaa8c6b237c6fd",
        "--index-url https://download.pytorch.org/whl/cpu",
        "torch==2.10.0",
        "workflow_validate.py capture",
        "workflow_validate.py validate",
        "--baseline",
        "--limit 10",
        ".complete == true",
        ".failures | length",
        ".valid == true",
        "git diff --name-only",
        "git ls-files --others --exclude-standard",
        "generation/workflow-candidates-",
        "Co-authored-by: Claude <noreply@anthropic.com>",
        "Watermark: Claude",
        "python3 -B -m unittest discover",
    ] {
        assert!(workflow.contains(required), "missing policy: {required}");
    }
    let pending = workflow.find("gh api --paginate").unwrap();
    let capture = workflow.find("workflow_validate.py capture").unwrap();
    let harvest = workflow.find("cargo workflow-harvest").unwrap();
    let validate = workflow.find("workflow_validate.py validate").unwrap();
    let diff = workflow.find("git diff --name-only").unwrap();
    let push = workflow.find("push origin").unwrap();
    let propose = workflow.find("gh pr create").unwrap();
    assert!(pending < capture && capture < harvest && harvest < validate);
    assert!(validate < diff && diff < push && push < propose);
    assert!(!workflow.contains("uses:"));
    assert!(!workflow.contains("pull_request_target"));
    let validator = fs::read_to_string(root.join("scripts/workflow_validate.py")).unwrap();
    assert!(validator.contains("execution.PromptQueue.put = forbid_execution"));
    assert!(validator.contains("execution.PromptExecutor.execute = forbid_execution"));
    assert!(!validator.contains("main.start_comfyui("));
    assert!(!validator.contains("/prompt"));
}
