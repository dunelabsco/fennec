use fennec::tools::files::{ListDirTool, ReadFileTool, WriteFileTool};
use fennec::tools::shell::ShellTool;
use fennec::tools::traits::Tool;
use tempfile::TempDir;

fn test_shell() -> ShellTool {
    ShellTool::new(
        vec!["echo".to_string(), "ls".to_string(), "cat".to_string()],
        vec!["/etc/shadow".to_string(), "/root".to_string()],
        10,
    )
}

#[tokio::test]
async fn shell_echo_works() {
    let shell = test_shell();
    let result = shell
        .execute(serde_json::json!({"command": "echo hello world"}))
        .await
        .expect("execute");
    assert!(result.success);
    assert_eq!(result.output.trim(), "hello world");
    assert!(result.error.is_none());
}

#[tokio::test]
async fn shell_blocks_disallowed_command() {
    let shell = test_shell();
    let result = shell
        .execute(serde_json::json!({"command": "rm -rf /"}))
        .await
        .expect("execute");
    assert!(!result.success);
    assert!(result.error.as_deref().unwrap().contains("not allowed"));
}

#[tokio::test]
async fn shell_blocks_forbidden_paths() {
    let shell = test_shell();
    let result = shell
        .execute(serde_json::json!({"command": "cat /etc/shadow"}))
        .await
        .expect("execute");
    assert!(!result.success);
    assert!(result.error.as_deref().unwrap().contains("forbidden path"));
}

#[tokio::test]
async fn read_file_works() {
    let dir = TempDir::new().expect("tempdir");
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "hello from file").expect("write");

    let tool = ReadFileTool::new();
    let result = tool
        .execute(serde_json::json!({"path": file_path.to_str().unwrap()}))
        .await
        .expect("execute");
    assert!(result.success);
    assert_eq!(result.output, "hello from file");
}

#[tokio::test]
async fn read_file_not_found() {
    let tool = ReadFileTool::new();
    let result = tool
        .execute(serde_json::json!({"path": "/tmp/nonexistent_fennec_test_file_12345.txt"}))
        .await
        .expect("execute");
    assert!(!result.success);
    assert!(result.error.as_deref().unwrap().contains("failed to read"));
}

#[tokio::test]
async fn write_file_works() {
    let dir = TempDir::new().expect("tempdir");
    let file_path = dir.path().join("subdir").join("output.txt");

    let tool = WriteFileTool::new();
    let result = tool
        .execute(serde_json::json!({
            "path": file_path.to_str().unwrap(),
            "content": "written content"
        }))
        .await
        .expect("execute");
    assert!(result.success);
    assert!(result.output.contains("15 bytes"));

    let content = std::fs::read_to_string(&file_path).expect("read back");
    assert_eq!(content, "written content");
}

#[tokio::test]
async fn list_dir_works() {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("a.txt"), "").expect("write a");
    std::fs::write(dir.path().join("b.txt"), "").expect("write b");
    std::fs::create_dir(dir.path().join("subdir")).expect("mkdir");

    let tool = ListDirTool::new();
    let result = tool
        .execute(serde_json::json!({"path": dir.path().to_str().unwrap()}))
        .await
        .expect("execute");
    assert!(result.success);
    assert!(result.output.contains("a.txt"));
    assert!(result.output.contains("b.txt"));
    assert!(result.output.contains("subdir/"));
}

#[test]
fn tool_spec_generation() {
    let tool = ReadFileTool::new();
    let spec = tool.spec();
    assert_eq!(spec.name, "read_file");
    assert!(!spec.description.is_empty());
    assert!(spec.parameters.is_object());

    let shell = test_shell();
    let spec = shell.spec();
    assert_eq!(spec.name, "shell");
}

#[test]
fn read_file_is_read_only() {
    let tool = ReadFileTool::new();
    assert!(tool.is_read_only());
}

#[test]
fn list_dir_is_read_only() {
    let tool = ListDirTool::new();
    assert!(tool.is_read_only());
}

#[test]
fn write_file_is_not_read_only() {
    let tool = WriteFileTool::new();
    assert!(!tool.is_read_only());
}

#[test]
fn shell_is_not_read_only() {
    let shell = test_shell();
    assert!(!shell.is_read_only());
}
