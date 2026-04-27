use std::path::Path;

use anyhow::{Context, Result};

use crate::memory::traits::{Memory, MemoryCategory, MemoryEntry};

/// Sentinel line written before each memory entry in a snapshot file.
///
/// The previous export/hydrate contract used `\n## ` as the entry
/// delimiter. That's ambiguous: any Core memory whose content itself
/// contained a Markdown H2 header (`## Something` on a line) would be
/// split into two fake entries at hydrate time, silently corrupting
/// the user's memory store after a round-trip.
///
/// An HTML comment is invisible in rendered Markdown, so snapshots
/// remain human-readable; the `fennec:entry` marker inside the comment
/// is specific enough that natural content will not collide with it.
const ENTRY_SENTINEL: &str = "<!-- fennec:entry -->";

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
        // Precede every entry with the sentinel so hydrate can split
        // unambiguously regardless of what's inside `content`.
        md.push_str(&format!(
            "\n{sentinel}\n## {key}\n{content}\n",
            sentinel = ENTRY_SENTINEL,
            key = entry.key,
            content = entry.content,
        ));
    }

    // Create parent dirs if needed.
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent)
                .await
                .context("creating parent dirs for snapshot")?;
        }
    }

    // Atomic write: write to a sibling temp file, then `rename` it on
    // top of `path`. POSIX rename(2) is atomic — no observer can see a
    // partially-written snapshot, even if the process crashes during
    // `tokio::fs::write`. The previous direct `tokio::fs::write` would
    // truncate the existing snapshot on open and then write incrementally;
    // a crash mid-write left zero bytes on disk, silently destroying
    // the user's memory snapshot.
    let tmp_path = match path.file_name() {
        Some(name) => {
            // Same directory as `path` so `rename` stays on the same
            // filesystem (otherwise it falls back to copy+delete which
            // is no longer atomic).
            let mut tmp = name.to_os_string();
            tmp.push(".tmp");
            path.with_file_name(tmp)
        }
        // Defensive fallback for paths without a filename component
        // (shouldn't happen for snapshots, but `path.with_file_name`
        // would otherwise panic on '.' or '/').
        None => path.with_extension("tmp"),
    };
    tokio::fs::write(&tmp_path, md.as_bytes())
        .await
        .context("writing snapshot tempfile")?;
    tokio::fs::rename(&tmp_path, path)
        .await
        .context("renaming snapshot tempfile into place")?;

    Ok(entries.len())
}

/// Hydrate Core memories from a Markdown snapshot file.
///
/// Parses entries preceded by the [`ENTRY_SENTINEL`] comment. For
/// backwards compatibility with snapshots written before the sentinel
/// was introduced, falls back to the old `\n## ` split when no
/// sentinel is present (and logs a WARN suggesting a re-export).
///
/// Returns the number of entries hydrated.
pub async fn hydrate_from_snapshot(memory: &dyn Memory, path: &Path) -> Result<usize> {
    let raw = tokio::fs::read_to_string(path)
        .await
        .context("reading snapshot file")?;

    let entries = parse_entries(&raw);
    let mut count = 0usize;
    for (key, content) in entries {
        if key.is_empty() {
            continue;
        }
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

/// Parse a snapshot's raw text into `(key, content)` pairs.
///
/// New format (preferred): each entry is preceded by
/// `"\n<!-- fennec:entry -->\n## key\n<content>\n"`.
///
/// Legacy format (pre-sentinel): each entry is `"\n## key\n<content>"`.
/// This is ambiguous when `<content>` contains a Markdown H2 header;
/// we still parse it for backward compat but the result may be garbled
/// if the ambiguity applies. A WARN is logged when we fall back.
fn parse_entries(raw: &str) -> Vec<(String, String)> {
    let delimiter = format!("\n{}\n", ENTRY_SENTINEL);
    if raw.contains(&delimiter) {
        return raw
            .split(&delimiter)
            .skip(1) // preamble
            .filter_map(|chunk| parse_one_entry(chunk))
            .collect();
    }

    // Legacy fallback.
    tracing::warn!(
        "Snapshot has no '{}' sentinels; falling back to legacy '\\n## ' split. \
         Re-export the snapshot to eliminate ambiguity with '##'-prefixed content.",
        ENTRY_SENTINEL
    );
    raw.split("\n## ")
        .skip(1) // preamble
        .filter_map(|chunk| parse_one_entry_legacy(chunk))
        .collect()
}

/// Parse one sentinel-delimited chunk. Shape:
/// `"## key\n<content>\n"` (with optional trailing blank line).
fn parse_one_entry(chunk: &str) -> Option<(String, String)> {
    let stripped = chunk.strip_prefix("## ")?;
    let mut lines = stripped.lines();
    let key = lines.next()?.trim().to_string();
    let content = lines.collect::<Vec<&str>>().join("\n").trim().to_string();
    Some((key, content))
}

/// Parse one legacy chunk (after splitting on `"\n## "`). Shape:
/// `"key\n<content>"`.
fn parse_one_entry_legacy(chunk: &str) -> Option<(String, String)> {
    let mut lines = chunk.lines();
    let key = lines.next()?.trim().to_string();
    let content = lines.collect::<Vec<&str>>().join("\n").trim().to_string();
    Some((key, content))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_entries_new_format() {
        let raw = "\
# Fennec Memory Snapshot
Generated: 2026-04-24T00:00:00+00:00

<!-- fennec:entry -->
## key1
content one

<!-- fennec:entry -->
## key2
content two
";
        let entries = parse_entries(raw);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, "key1");
        assert_eq!(entries[0].1, "content one");
        assert_eq!(entries[1].0, "key2");
        assert_eq!(entries[1].1, "content two");
    }

    /// Regression: an entry whose content contains its own `## header`
    /// line previously got split into two fake entries at hydrate time.
    /// With the sentinel delimiter it stays intact.
    #[test]
    fn parse_entries_preserves_h2_in_content() {
        let raw = "\
# Snapshot
Generated: ts

<!-- fennec:entry -->
## config
Here are some notes:
## subheading that used to split
more content here
";
        let entries = parse_entries(raw);
        assert_eq!(entries.len(), 1, "entries: {:?}", entries);
        assert_eq!(entries[0].0, "config");
        assert!(
            entries[0].1.contains("subheading that used to split"),
            "content truncated: {:?}",
            entries[0].1
        );
    }

    /// Back-compat: a legacy snapshot (no sentinel) still parses. The
    /// H2-in-content problem is still present in legacy mode — users
    /// are expected to re-export once to upgrade.
    #[test]
    fn parse_entries_legacy_format_still_works() {
        let raw = "\
# Snapshot
Generated: ts

## key1
content one

## key2
content two
";
        let entries = parse_entries(raw);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, "key1");
        assert_eq!(entries[1].0, "key2");
    }

    #[test]
    fn parse_entries_empty_preamble_only() {
        let raw = "# Snapshot\nGenerated: ts\n";
        let entries = parse_entries(raw);
        assert!(entries.is_empty());
    }

    /// Export-then-hydrate-via-parse round-trip with `##` inside content
    /// — catches regressions in the sentinel-format writer.
    #[test]
    fn export_format_round_trips_h2_content() {
        let timestamp = "2026-04-24T00:00:00+00:00";
        let mut md = format!("# Fennec Memory Snapshot\nGenerated: {timestamp}\n");
        // Simulate what export_snapshot writes for one entry.
        md.push_str(&format!(
            "\n{sentinel}\n## weird-key\n{content}\n",
            sentinel = ENTRY_SENTINEL,
            content = "Before\n## Inner Header\nAfter",
        ));
        let entries = parse_entries(&md);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "weird-key");
        assert!(entries[0].1.contains("## Inner Header"));
    }
}
