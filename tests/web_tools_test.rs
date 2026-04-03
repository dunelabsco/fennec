use fennec::tools::traits::Tool;
use fennec::tools::web::{WebFetchTool, WebSearchTool};

#[test]
fn web_fetch_spec() {
    let tool = WebFetchTool::new();
    let spec = tool.spec();
    assert_eq!(spec.name, "web_fetch");
    assert!(!spec.description.is_empty());
    assert!(spec.parameters.is_object());

    let props = spec.parameters.get("properties").expect("has properties");
    assert!(props.get("url").is_some());
    assert!(props.get("max_length").is_some());

    let required = spec.parameters.get("required").expect("has required");
    let required_arr = required.as_array().expect("is array");
    assert!(required_arr.iter().any(|v| v.as_str() == Some("url")));
}

#[test]
fn web_search_spec() {
    let tool = WebSearchTool::new();
    let spec = tool.spec();
    assert_eq!(spec.name, "web_search");
    assert!(!spec.description.is_empty());
    assert!(spec.parameters.is_object());

    let props = spec.parameters.get("properties").expect("has properties");
    assert!(props.get("query").is_some());
    assert!(props.get("num_results").is_some());

    let required = spec.parameters.get("required").expect("has required");
    let required_arr = required.as_array().expect("is array");
    assert!(required_arr.iter().any(|v| v.as_str() == Some("query")));
}

#[test]
fn web_fetch_is_read_only() {
    let tool = WebFetchTool::new();
    assert!(tool.is_read_only());
}

#[test]
fn web_search_is_read_only() {
    let tool = WebSearchTool::new();
    assert!(tool.is_read_only());
}

#[tokio::test]
async fn web_fetch_missing_url_param() {
    let tool = WebFetchTool::new();
    let result = tool
        .execute(serde_json::json!({}))
        .await
        .expect("execute");
    assert!(!result.success);
    assert!(
        result.error.as_deref().unwrap().contains("missing required parameter"),
        "error should mention missing parameter, got: {:?}",
        result.error
    );
}

#[tokio::test]
async fn web_search_missing_query_param() {
    let tool = WebSearchTool::new();
    let result = tool
        .execute(serde_json::json!({}))
        .await
        .expect("execute");
    assert!(!result.success);
    assert!(
        result.error.as_deref().unwrap().contains("missing required parameter"),
        "error should mention missing parameter, got: {:?}",
        result.error
    );
}
