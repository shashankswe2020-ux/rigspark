use rigspark_core::catalog_request::{ModelRequest, Repository};
use serde_json::json;

#[test]
fn canonical_hugging_face_repository_urls_are_normalized() {
    let repository =
        Repository::parse("https://huggingface.co/JetBrains/Mellum2.1-12B-A2.5B-Thinking-GGUF")
            .unwrap();
    assert_eq!(
        repository.as_str(),
        "JetBrains/Mellum2.1-12B-A2.5B-Thinking-GGUF"
    );
    assert_eq!(
        repository.url(),
        "https://huggingface.co/JetBrains/Mellum2.1-12B-A2.5B-Thinking-GGUF"
    );
}

#[test]
fn alternate_hosts_paths_and_url_features_are_rejected() {
    for raw in [
        "http://huggingface.co/owner/model",
        "https://hf.co/owner/model",
        "https://user@huggingface.co/owner/model",
        "https://huggingface.co:443/owner/model",
        "https://huggingface.co/owner/model/tree/main",
        "https://huggingface.co/owner/model?download=true",
        "https://huggingface.co/owner/model#readme",
        "https://huggingface.co/owner",
        "https://huggingface.co/owner/model/extra",
        "https://huggingface.co/owner/model name",
    ] {
        assert!(Repository::parse(raw).is_err(), "{raw}");
    }
}

#[test]
fn issue_form_event_extracts_only_the_model_url_field() {
    let event = json!({
        "issue": {
            "number": 321,
            "html_url": "https://github.com/shashankswe2020-ux/rigspark/issues/321",
            "body": "### Hugging Face model URL\n\nhttps://huggingface.co/owner/model-GGUF\n\n### Reason or use case\n\nPlease add it."
        }
    });
    let request = ModelRequest::from_event(&event.to_string()).unwrap();
    assert_eq!(request.issue_number, 321);
    assert_eq!(request.repository.as_str(), "owner/model-GGUF");
    assert_eq!(
        request.issue_url,
        "https://github.com/shashankswe2020-ux/rigspark/issues/321"
    );
}

#[test]
fn malformed_or_oversized_issue_events_fail_closed() {
    for event in [
        json!({}),
        json!({"issue":{"number":0,"html_url":"https://github.com/shashankswe2020-ux/rigspark/issues/1","body":"### Hugging Face model URL\n\nhttps://huggingface.co/owner/model"}}),
        json!({"issue":{"number":1,"html_url":"https://example.com/issues/1","body":"### Hugging Face model URL\n\nhttps://huggingface.co/owner/model"}}),
        json!({"issue":{"number":1,"html_url":"https://github.com/shashankswe2020-ux/rigspark/issues/1","body":"https://huggingface.co/owner/model"}}),
        json!({"issue":{"number":1,"html_url":"https://github.com/shashankswe2020-ux/rigspark/issues/1","body":"### Hugging Face model URL\n\nhttps://huggingface.co/owner/model\nhttps://huggingface.co/other/model"}}),
    ] {
        assert!(ModelRequest::from_event(&event.to_string()).is_err());
    }
    assert!(ModelRequest::from_event(&"x".repeat(70 * 1024)).is_err());
}
