use std::path::Path;

use anyhow::{Context, Result};

use crate::memory::traits::{Memory, MemoryCategory, MemoryEntry};

/// Export all Core memories to a Markdown snapshot file.
///
/// Returns the number of entries written.
pub async fn export_snapshot(memory: &dyn Memory, path: &Path) -> Result<usize> {
    let entries = memory
        .list(Some(&MemoryCategory::Core), usize::MAX)
        .await
        .context("listing core memories for snapshot")?;

    let timestamp = chrono::Utc::now().to_rfc3339();
    let mut md = format!("# Fennec Memory Snapshot\nGenerated: {timestamp}\n");

    for entry in &entries {
        md.push_str(&format!("\n## {}\n{}\n", entry.key, entry.content));
    }

    // Create parent dirs if needed.
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent)
                .await
                .context("creating parent dirs for snapshot")?;
        }
    }

    tokio::fs::write(path, md.as_bytes())
        .await
        .context("writing snapshot file")?;

    Ok(entries.len())
}

/// Hydrate Core memories from a Markdown snapshot file.
///
/// Parses `## key` headers and the content below each one, storing them
/// as Core memories. Returns the number of entries hydrated.
pub async fn hydrate_from_snapshot(memory: &dyn Memory, path: &Path) -> Result<usize> {
    let raw = tokio::fs::read_to_string(path)
        .await
        .context("reading snapshot file")?;

    let mut count = 0usize;

    // Split on "## " headers. The first chunk is the preamble (title line, etc.).
    let sections: Vec<&str> = raw.split("\n## ").collect();

    for section in sections.iter().skip(1) {
        // First line is the key, rest is content.
        let mut lines = section.lines();
        let key = match lines.next() {
            Some(k) => k.trim().to_string(),
            None => continue,
        };
        if key.is_empty() {
            continue;
        }

        let content: String = lines.collect::<Vec<&str>>().join("\n").trim().to_string();

        let now = chrono::Utc::now().to_rfc3339();
        let entry = MemoryEntry {
            key,
            content,
            category: MemoryCategory::Core,
            created_at: now.clone(),
            updated_at: now,
            ..MemoryEntry::default()
        };

        memory.store(entry).await.context("storing hydrated entry")?;
        count += 1;
    }

    Ok(count)
}
