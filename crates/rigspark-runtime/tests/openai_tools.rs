use rigspark_runtime::ollama_inference::{ChatMessage, ToolCall};
use rigspark_runtime::openai::{chat_result, openai_messages};
use serde_json::json;
use std::collections::BTreeMap;

fn msg(role: &str, content: &str) -> ChatMessage {
    ChatMessage {
        role: role.into(),
        content: content.into(),
        tool_calls: Vec::new(),
        tool_name: None,
    }
}

#[test]
fn tool_calls_come_back_from_llama_cpp_replies() {
    let reply = chat_result(json!({"choices":[{"message":{"content":null,"tool_calls":[
        {"id":"x","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"src/a.ts\"}"}},
        {"id":"y","type":"function","function":{"name":"list_files","arguments":{"path":"src"}}},
        {"id":"z","type":"function","function":{"name":"status","arguments":""}}
    ]}}]}))
    .unwrap();
    assert_eq!(reply.content, "");
    let names: Vec<_> = reply.tool_calls.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["read_file", "list_files", "status"]);
    assert_eq!(reply.tool_calls[0].arguments["path"], "src/a.ts");
    assert_eq!(reply.tool_calls[1].arguments["path"], "src");
    assert!(reply.tool_calls[2].arguments.is_empty());

    let plain = chat_result(json!({"choices":[{"message":{"content":"hi"}}]})).unwrap();
    assert_eq!(plain.content, "hi");
    assert!(plain.tool_calls.is_empty());
}

#[test]
fn bad_tool_calls_are_refused() {
    for bad in [json!("not json"), json!("[1,2]"), json!(42)] {
        let value = json!({"choices":[{"message":{"content":"","tool_calls":[
            {"function":{"name":"read_file","arguments":bad}}]}}]});
        assert!(chat_result(value).is_err());
    }
    let unnamed =
        json!({"choices":[{"message":{"tool_calls":[{"function":{"name":"","arguments":"{}"}}]}}]});
    assert!(chat_result(unnamed).is_err());
}

#[test]
fn history_with_tool_calls_is_sent_in_openai_shape() {
    let mut args = BTreeMap::new();
    args.insert("path".to_string(), json!("src/a.ts"));
    let mut assistant = msg("assistant", "");
    assistant.tool_calls = vec![
        ToolCall {
            name: "read_file".into(),
            arguments: args,
        },
        ToolCall {
            name: "list_files".into(),
            arguments: BTreeMap::new(),
        },
    ];
    let mut first = msg("tool", "export const a = 1;");
    first.tool_name = Some("read_file".into());
    let mut second = msg("tool", "a.ts b.ts");
    second.tool_name = Some("list_files".into());
    let sent = openai_messages(&[
        msg("system", "rules"),
        msg("user", "fix it"),
        assistant,
        first,
        second,
    ]);
    assert_eq!(sent[2]["tool_calls"][0]["id"], "call_2_0");
    assert_eq!(sent[2]["tool_calls"][0]["type"], "function");
    assert_eq!(
        sent[2]["tool_calls"][0]["function"]["arguments"],
        "{\"path\":\"src/a.ts\"}"
    );
    assert_eq!(sent[3]["tool_call_id"], "call_2_0");
    assert_eq!(sent[3]["name"], "read_file");
    assert_eq!(sent[4]["tool_call_id"], "call_2_1");
    assert!(sent[1].get("tool_call_id").is_none() && sent[1].get("tool_calls").is_none());
}
