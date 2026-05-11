//! Built-in slash commands registered at TUI startup.
//!
//! Each command is its own zero-sized struct implementing
//! `CommandHandler`. The set here is the F1-1 must-have list
//! lifted from the upstream's TUI inventory: every command a
//! daily user reaches for in the first month.
//!
//! Power-user / overlay-heavy commands (`/agents`, `/replay`,
//! `/rollback`, `/browser`) live in F1-2. Stubs in here that
//! return `NotImplemented` are deliberate placeholders for
//! features whose real implementation needs infrastructure
//! that's still being landed (session store wiring, MCP reload,
//! voice modal UI, etc.) — they ship as discoverable surface
//! so the user sees the command in `/help` and gets a real
//! answer about its status.

use anyhow::Result;

use super::super::app::{App, ChatLine};
use super::{AgentAction, CommandHandler, CommandOutcome, CommandRegistry};

/// Register every F1-1 built-in command on `r`.
pub fn register_all(r: &mut CommandRegistry) {
    r.register(Box::new(Help));
    r.register(Box::new(Quit));
    r.register(Box::new(Clear));
    r.register(Box::new(Resume));
    r.register(Box::new(Title));
    r.register(Box::new(Save));
    r.register(Box::new(History));
    r.register(Box::new(Compact));
    r.register(Box::new(Details));
    r.register(Box::new(Redraw));
    r.register(Box::new(Mouse));
    r.register(Box::new(Model));
    r.register(Box::new(Voice));
    r.register(Box::new(Usage));
    r.register(Box::new(Status));
    r.register(Box::new(Steer));
    r.register(Box::new(Undo));
    r.register(Box::new(Retry));
    r.register(Box::new(Skills));
    r.register(Box::new(Tools));
    r.register(Box::new(Reload));
    r.register(Box::new(ReloadMcp));
    r.register(Box::new(Image));
    r.register(Box::new(Paste));
    r.register(Box::new(Copy));
    r.register(Box::new(Agents));
    // F1-2-E commands.
    r.register(Box::new(Fortune));
    r.register(Box::new(Queue));
    r.register(Box::new(StatusBar));
    r.register(Box::new(Logs));
    r.register(Box::new(Indicator));
    r.register(Box::new(ReloadSkills));
    r.register(Box::new(Verbose));
    r.register(Box::new(Busy));
    r.register(Box::new(Reasoning));
    r.register(Box::new(Personality));
    r.register(Box::new(Branch));
    r.register(Box::new(Replay));
    r.register(Box::new(ReplayDiff));
}

// -- Lifecycle / focus -------------------------------------------

struct Help;
impl CommandHandler for Help {
    fn name(&self) -> &'static str {
        "help"
    }
    fn help(&self) -> &'static str {
        "show all commands"
    }
    fn execute(&self, _args: &str, app: &mut App) -> Result<CommandOutcome> {
        let mut body = String::from("commands:\n");
        for (cmd, desc) in HELP_ENTRIES {
            body.push_str(&format!("  /{cmd:<14} {desc}\n"));
        }
        body.push_str(
            "\nkeyboard: [tab] next pane · [↑↓] history · [shift-enter] newline · \
             [ctrl-c] quit · [ctrl-z/y] undo/redo · [ctrl-w/u/k] word/line delete",
        );
        push_system(app, body);
        Ok(CommandOutcome::Status("ok".into()))
    }
}

const HELP_ENTRIES: &[(&str, &str)] = &[
    ("help", "show this list"),
    ("quit", "exit the TUI"),
    ("clear / new", "start a fresh conversation"),
    ("resume <id>", "switch to a saved session"),
    ("title [name]", "get or set the session title"),
    ("save", "save the transcript to disk"),
    ("history [n]", "show the last n turns inline"),
    ("compact", "toggle compact rendering"),
    ("details", "toggle thinking / tool detail visibility"),
    ("redraw", "force a full repaint"),
    ("mouse on|off", "toggle scroll-wheel + click tracking"),
    ("model [name]", "switch the active LLM"),
    ("voice on|off|status", "voice input mode (placeholder)"),
    ("usage", "show this session's token usage"),
    ("status", "show gateway + agent state"),
    ("steer <prompt>", "inject a steer note for the next turn"),
    ("undo", "remove the last user / assistant pair"),
    ("retry", "re-run the last user message"),
    ("skills", "browse / inspect skills"),
    ("tools enable|disable <name>", "toggle a tool"),
    ("reload", "reload .env into the running gateway"),
    ("reload-mcp", "rescan MCP servers"),
    ("image <path>", "attach an image to the next message"),
    ("paste", "paste clipboard text into the input"),
    ("copy [n]", "copy a past assistant message to clipboard"),
    ("agents", "open the spawn-tree dashboard (/agents pause|resume|status)"),
    ("fortune", "show a random / daily fortune"),
    ("queue [msg]", "enqueue a message for the next turn"),
    ("statusbar / sb", "toggle status bar (top|bottom|off|toggle)"),
    ("logs [n]", "show the last n tracing lines"),
    ("indicator", "spinner style (braille|ascii|kaomoji|emoji|unicode)"),
    ("reload-skills", "rescan ~/.fennec/skills and refresh prompt"),
    ("verbose", "tool-output verbosity (on|off|cycle)"),
    ("busy", "Enter mid-turn behaviour (interrupt|queue|steer|status)"),
    ("reasoning", "thinking effort + visibility (off|low|medium|high|xhigh|hide|show)"),
    ("personality", "swap persona name (empty resets to default)"),
    ("branch / fork", "fork the current session into a new row"),
    ("replay", "open spawn-tree history (N|last|list|load)"),
    ("replay-diff", "diff two spawn-tree snapshots in the overlay"),
];

struct Quit;
impl CommandHandler for Quit {
    fn name(&self) -> &'static str {
        "quit"
    }
    fn help(&self) -> &'static str {
        "exit the TUI"
    }
    fn aliases(&self) -> &[&'static str] {
        &["exit", "q"]
    }
    fn execute(&self, _args: &str, app: &mut App) -> Result<CommandOutcome> {
        app.should_quit = true;
        Ok(CommandOutcome::Quit)
    }
}

struct Redraw;
impl CommandHandler for Redraw {
    fn name(&self) -> &'static str {
        "redraw"
    }
    fn help(&self) -> &'static str {
        "force a full repaint"
    }
    fn execute(&self, _args: &str, _app: &mut App) -> Result<CommandOutcome> {
        // We render every frame; this is a no-op affordance for
        // users coming from upstream-style TUIs.
        Ok(CommandOutcome::Status("redrawn".into()))
    }
}

// -- Session ----------------------------------------------------

struct Clear;
impl CommandHandler for Clear {
    fn name(&self) -> &'static str {
        "clear"
    }
    fn help(&self) -> &'static str {
        "start a new conversation"
    }
    fn aliases(&self) -> &[&'static str] {
        &["new"]
    }
    fn execute(&self, _args: &str, app: &mut App) -> Result<CommandOutcome> {
        app.chat.clear();
        app.live_tool = None;
        Ok(CommandOutcome::Agent(AgentAction::Clear))
    }
}

struct Resume;
impl CommandHandler for Resume {
    fn name(&self) -> &'static str {
        "resume"
    }
    fn help(&self) -> &'static str {
        "resume a prior session"
    }
    fn execute(&self, args: &str, _app: &mut App) -> Result<CommandOutcome> {
        let target = args.trim();
        if target.is_empty() {
            return Ok(CommandOutcome::Status(
                "usage: /resume <session-id-or-title>".into(),
            ));
        }
        Ok(CommandOutcome::Agent(AgentAction::SessionResume(
            target.to_string(),
        )))
    }
}

struct Title;
impl CommandHandler for Title {
    fn name(&self) -> &'static str {
        "title"
    }
    fn help(&self) -> &'static str {
        "set or show the current session title"
    }
    fn execute(&self, args: &str, _app: &mut App) -> Result<CommandOutcome> {
        let trimmed = args.trim();
        let payload = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
        Ok(CommandOutcome::Agent(AgentAction::SessionTitle(payload)))
    }
}

struct Save;
impl CommandHandler for Save {
    fn name(&self) -> &'static str {
        "save"
    }
    fn help(&self) -> &'static str {
        "write the transcript to disk"
    }
    fn execute(&self, _args: &str, app: &mut App) -> Result<CommandOutcome> {
        let path = std::env::temp_dir().join(format!(
            "fennec-transcript-{}.txt",
            chrono::Local::now().format("%Y%m%d-%H%M%S")
        ));
        let mut body = String::new();
        for line in &app.chat {
            match line {
                ChatLine::System { time, body: b } => body.push_str(&format!("[sys {time}] {b}\n")),
                ChatLine::User { time, body: b } => body.push_str(&format!("[you {time}] {b}\n")),
                ChatLine::Bot { time, body: b } => body.push_str(&format!("[fennec {time}] {b}\n")),
                ChatLine::ToolCall { call } => body.push_str(&format!("    > tool: {call}\n")),
                ChatLine::ToolResult { summary } => body.push_str(&format!("      {summary}\n")),
                ChatLine::ToolRunning { label, .. } => body.push_str(&format!("    > {label}\n")),
            }
        }
        match std::fs::write(&path, body) {
            Ok(()) => push_system(app, format!("transcript saved to {}", path.display())),
            Err(e) => push_system(app, format!("save failed: {e}")),
        }
        Ok(CommandOutcome::Status("saved".into()))
    }
}

struct History;
impl CommandHandler for History {
    fn name(&self) -> &'static str {
        "history"
    }
    fn help(&self) -> &'static str {
        "show recent turns inline"
    }
    fn execute(&self, args: &str, app: &mut App) -> Result<CommandOutcome> {
        let n: usize = args.trim().parse().unwrap_or(10);
        let total = app.input.history.len();
        let mut body = format!("input history (last {} of {}):\n", n.min(total), total);
        for (i, h) in app.input.history.iter().take(n).enumerate() {
            body.push_str(&format!("  {i:>2}. {}\n", first_line(h, 80)));
        }
        push_system(app, body);
        Ok(CommandOutcome::Status("ok".into()))
    }
}

// -- Render toggles ---------------------------------------------

struct Compact;
impl CommandHandler for Compact {
    fn name(&self) -> &'static str {
        "compact"
    }
    fn help(&self) -> &'static str {
        "toggle compact transcript rendering (persists to config)"
    }
    fn execute(&self, args: &str, app: &mut App) -> Result<CommandOutcome> {
        let want = parse_toggle(args, app.compact_mode);
        app.compact_mode = want;
        push_system(
            app,
            format!("compact: {}", if want { "on" } else { "off" }),
        );
        // Snapshot to config.toml so the toggle survives a
        // restart — the submit loop handles the disk write so
        // the command stays sync.
        Ok(CommandOutcome::Agent(AgentAction::PersistTuiSettings))
    }
}

struct Details;
impl CommandHandler for Details {
    fn name(&self) -> &'static str {
        "details"
    }
    fn help(&self) -> &'static str {
        "set tool / reasoning detail visibility (hidden|collapsed|expanded)"
    }
    fn execute(&self, args: &str, app: &mut App) -> Result<CommandOutcome> {
        use crate::tui::app::DetailsMode;
        let trimmed = args.trim();
        if trimmed.is_empty() {
            // Cycle through the three modes — gives users a
            // single-keystroke toggle without remembering the
            // mode names.
            app.details_mode = match app.details_mode {
                DetailsMode::Expanded => DetailsMode::Collapsed,
                DetailsMode::Collapsed => DetailsMode::Hidden,
                DetailsMode::Hidden => DetailsMode::Expanded,
            };
        } else {
            match DetailsMode::parse(trimmed) {
                Some(m) => app.details_mode = m,
                None => {
                    return Ok(CommandOutcome::Status(format!(
                        "details: unknown mode '{trimmed}' (expected hidden/collapsed/expanded)"
                    )));
                }
            }
        }
        push_system(app, format!("details: {}", app.details_mode.as_str()));
        Ok(CommandOutcome::Agent(AgentAction::PersistTuiSettings))
    }
}

struct Mouse;
impl CommandHandler for Mouse {
    fn name(&self) -> &'static str {
        "mouse"
    }
    fn help(&self) -> &'static str {
        "toggle scroll-wheel + click tracking"
    }
    fn execute(&self, args: &str, app: &mut App) -> Result<CommandOutcome> {
        let want = parse_toggle(args, app.mouse_enabled);
        app.mouse_enabled = want;
        Ok(CommandOutcome::Status(format!(
            "mouse: {}",
            if want { "on" } else { "off" }
        )))
    }
}

// -- Agent state queries / actions ------------------------------

struct Status;
impl CommandHandler for Status {
    fn name(&self) -> &'static str {
        "status"
    }
    fn help(&self) -> &'static str {
        "show gateway + agent state"
    }
    fn execute(&self, _args: &str, app: &mut App) -> Result<CommandOutcome> {
        let connected = app.channels.len();
        push_system(
            app,
            format!(
                "fennec v{} · sessions {} · channels {} · scroll {}",
                env!("CARGO_PKG_VERSION"),
                app.sessions.len(),
                connected,
                app.chat_scroll
            ),
        );
        Ok(CommandOutcome::Status("ok".into()))
    }
}

struct Usage;
impl CommandHandler for Usage {
    fn name(&self) -> &'static str {
        "usage"
    }
    fn help(&self) -> &'static str {
        "session usage (live counts)"
    }
    fn execute(&self, _args: &str, _app: &mut App) -> Result<CommandOutcome> {
        // Real rendering happens in main.rs after the agent lock
        // can be acquired — token totals live on the Agent itself
        // and the slash dispatch path holds the parking_lot App
        // mutex, not the tokio Agent mutex.
        Ok(CommandOutcome::Agent(AgentAction::ShowUsage))
    }
}

struct Steer;
impl CommandHandler for Steer {
    fn name(&self) -> &'static str {
        "steer"
    }
    fn help(&self) -> &'static str {
        "inject steering note for the next turn"
    }
    fn execute(&self, args: &str, app: &mut App) -> Result<CommandOutcome> {
        let note = args.trim();
        if note.is_empty() {
            push_system(app, "steer needs a note. usage: /steer <text>".into());
            return Ok(CommandOutcome::Status("missing arg".into()));
        }
        push_system(app, format!("steer queued: {note}"));
        Ok(CommandOutcome::Agent(AgentAction::Steer(note.to_string())))
    }
}

struct Undo;
impl CommandHandler for Undo {
    fn name(&self) -> &'static str {
        "undo"
    }
    fn help(&self) -> &'static str {
        "remove the last user / assistant pair"
    }
    fn execute(&self, _args: &str, app: &mut App) -> Result<CommandOutcome> {
        // Pop the most recent user and bot lines from the chat.
        // The agent-side undo (history pop) is dispatched as an
        // AgentAction.
        let mut popped = 0;
        while popped < 4 && !app.chat.is_empty() {
            let last_is_terminal = matches!(
                app.chat.last(),
                Some(ChatLine::User { .. }) | Some(ChatLine::Bot { .. })
            );
            app.chat.pop();
            popped += 1;
            if last_is_terminal && popped >= 2 {
                break;
            }
        }
        Ok(CommandOutcome::Agent(AgentAction::Undo))
    }
}

struct Retry;
impl CommandHandler for Retry {
    fn name(&self) -> &'static str {
        "retry"
    }
    fn help(&self) -> &'static str {
        "re-run the last user message"
    }
    fn execute(&self, _args: &str, app: &mut App) -> Result<CommandOutcome> {
        let last = app
            .input
            .history
            .front()
            .cloned()
            .unwrap_or_default();
        if last.is_empty() {
            push_system(app, "no prior message to retry.".into());
            return Ok(CommandOutcome::Status("nothing to retry".into()));
        }
        Ok(CommandOutcome::Agent(AgentAction::Retry))
    }
}

struct Model;
impl CommandHandler for Model {
    fn name(&self) -> &'static str {
        "model"
    }
    fn help(&self) -> &'static str {
        "show or switch the active LLM"
    }
    fn execute(&self, args: &str, _app: &mut App) -> Result<CommandOutcome> {
        let arg = args.trim();
        let payload = if arg.is_empty() {
            None
        } else {
            Some(arg.to_string())
        };
        Ok(CommandOutcome::Agent(AgentAction::SwitchModel(payload)))
    }
}

// -- Voice / Skills / Tools / Reload ----------------------------

struct Voice;
impl CommandHandler for Voice {
    fn name(&self) -> &'static str {
        "voice"
    }
    fn help(&self) -> &'static str {
        "voice input mode — `/voice` toggles, `/voice tts on|off`, `/voice status`"
    }
    fn execute(&self, args: &str, app: &mut App) -> Result<CommandOutcome> {
        use super::super::voice::VoiceState;
        let arg = args.trim().to_lowercase();
        match arg.as_str() {
            "" | "toggle" => match app.voice.state() {
                VoiceState::Idle => {
                    app.voice.start_recording();
                    Ok(CommandOutcome::Status("● recording — /voice to stop".into()))
                }
                VoiceState::Recording => {
                    let voice_dir = std::env::temp_dir().join("fennec-voice");
                    match app.voice.stop_recording(&voice_dir) {
                        Ok(_path) => Ok(CommandOutcome::Status(
                            "transcribing…".into(),
                        )),
                        Err(e) => {
                            push_system(app, format!("voice stop failed: {e}"));
                            Ok(CommandOutcome::Status("error".into()))
                        }
                    }
                }
                VoiceState::Transcribing => Ok(CommandOutcome::Status(
                    "still transcribing — wait a moment".into(),
                )),
            },
            "on" => {
                app.voice.start_recording();
                Ok(CommandOutcome::Status("● recording".into()))
            }
            "off" => {
                let voice_dir = std::env::temp_dir().join("fennec-voice");
                match app.voice.stop_recording(&voice_dir) {
                    Ok(_) => Ok(CommandOutcome::Status("transcribing…".into())),
                    Err(e) => {
                        push_system(app, format!("voice stop failed: {e}"));
                        Ok(CommandOutcome::Status("error".into()))
                    }
                }
            }
            "tts" => {
                let on = !app.voice.tts_enabled();
                app.voice.set_tts(on);
                Ok(CommandOutcome::Status(format!(
                    "tts: {}",
                    if on { "on" } else { "off" }
                )))
            }
            "tts on" => {
                app.voice.set_tts(true);
                Ok(CommandOutcome::Status("tts: on".into()))
            }
            "tts off" => {
                app.voice.set_tts(false);
                Ok(CommandOutcome::Status("tts: off".into()))
            }
            "status" => {
                let state = match app.voice.state() {
                    VoiceState::Idle => "idle",
                    VoiceState::Recording => "● recording",
                    VoiceState::Transcribing => "transcribing",
                };
                let tts = if app.voice.tts_enabled() { "on" } else { "off" };
                push_system(
                    app,
                    format!("voice: {state} · tts: {tts}"),
                );
                Ok(CommandOutcome::Status("ok".into()))
            }
            other => {
                push_system(
                    app,
                    format!(
                        "unknown /voice arg: {other:?}. usage: /voice [on|off|toggle|tts|status]"
                    ),
                );
                Ok(CommandOutcome::Status("?".into()))
            }
        }
    }
}

struct Skills;
impl CommandHandler for Skills {
    fn name(&self) -> &'static str {
        "skills"
    }
    fn help(&self) -> &'static str {
        "list / search / inspect installed skills"
    }
    fn execute(&self, args: &str, app: &mut App) -> Result<CommandOutcome> {
        let trimmed = args.trim();
        let skills_dir = match app.skills_dir.clone() {
            Some(d) => d,
            None => {
                return Ok(CommandOutcome::Status(
                    "skills directory not configured (no home dir)".into(),
                ));
            }
        };

        let skills = match crate::skills::SkillsLoader::load_from_directory(&skills_dir) {
            Ok(s) => s,
            Err(e) => {
                push_system(app, format!("skills load failed: {e}"));
                return Ok(CommandOutcome::Status("error".into()));
            }
        };

        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let verb = parts.next().unwrap_or("").to_lowercase();
        let rest = parts.next().unwrap_or("").trim();

        match verb.as_str() {
            "" | "list" => {
                if skills.is_empty() {
                    push_system(
                        app,
                        format!(
                            "no skills installed at {}. Drop a Markdown file with YAML frontmatter (name, description) to add one.",
                            skills_dir.display()
                        ),
                    );
                } else {
                    let mut body = format!("Skills ({} installed)\n", skills.len());
                    for s in &skills {
                        body.push_str(&format!(
                            "  · {}{}\n      {}\n",
                            s.name,
                            if s.always { " (always)" } else { "" },
                            truncate_inline(&s.description, 100),
                        ));
                    }
                    body.push_str("\nUse /skills inspect <name> for full details, /skills search <q> to filter.");
                    push_system(app, body.trim_end().to_string());
                }
            }
            "search" => {
                if rest.is_empty() {
                    return Ok(CommandOutcome::Status(
                        "usage: /skills search <query>".into(),
                    ));
                }
                let q = rest.to_lowercase();
                let hits: Vec<&crate::skills::Skill> = skills
                    .iter()
                    .filter(|s| {
                        s.name.to_lowercase().contains(&q)
                            || s.description.to_lowercase().contains(&q)
                    })
                    .collect();
                if hits.is_empty() {
                    push_system(app, format!("no skills match: {rest}"));
                } else {
                    let mut body =
                        format!("Skills matching '{rest}' ({} hit(s))\n", hits.len());
                    for s in &hits {
                        body.push_str(&format!(
                            "  · {}\n      {}\n",
                            s.name,
                            truncate_inline(&s.description, 100),
                        ));
                    }
                    push_system(app, body.trim_end().to_string());
                }
            }
            "inspect" => {
                if rest.is_empty() {
                    return Ok(CommandOutcome::Status(
                        "usage: /skills inspect <name>".into(),
                    ));
                }
                let needle = rest.to_lowercase();
                match skills.iter().find(|s| s.name.to_lowercase() == needle) {
                    Some(s) => {
                        let mut body = format!("Skill: {}\n", s.name);
                        body.push_str(&format!("Always-on: {}\n", s.always));
                        if !s.requirements.is_empty() {
                            body.push_str(&format!(
                                "Requirements: {}\n",
                                s.requirements.join(", ")
                            ));
                        }
                        body.push_str(&format!("\n{}\n\n", s.description));
                        // Snippet of body content so the user sees what's
                        // actually injected without needing to open the
                        // file.
                        let snippet = truncate_inline(&s.content, 800);
                        body.push_str("---\n");
                        body.push_str(&snippet);
                        push_system(app, body.trim_end().to_string());
                    }
                    None => {
                        push_system(app, format!("unknown skill: {rest}"));
                    }
                }
            }
            "install" | "browse" => {
                push_system(
                    app,
                    format!(
                        "/skills {verb}: routes through the skills hub (multi-source registry — \
                         GitHub / well-known / URL / community). The hub adapter set lands in \
                         a separate skills-hub PR; until then drop a Markdown file in {} to \
                         install manually.",
                        skills_dir.display()
                    ),
                );
            }
            other => {
                return Ok(CommandOutcome::Status(format!(
                    "/skills: unknown verb '{other}' (expected list/search/inspect/install/browse)"
                )));
            }
        }
        Ok(CommandOutcome::Status("ok".into()))
    }
}

/// Truncate to `max` chars with an ellipsis. Used by /skills
/// rendering — descriptions / content are often multi-line so
/// we collapse newlines first.
fn truncate_inline(s: &str, max: usize) -> String {
    let single: String = s.replace('\n', " ").chars().collect();
    if single.chars().count() <= max {
        return single;
    }
    let mut out: String = single.chars().take(max - 1).collect();
    out.push('…');
    out
}

struct Tools;
impl CommandHandler for Tools {
    fn name(&self) -> &'static str {
        "tools"
    }
    fn help(&self) -> &'static str {
        "list/enable/disable tools (chat history clears on change)"
    }
    fn execute(&self, args: &str, _app: &mut App) -> Result<CommandOutcome> {
        let trimmed = args.trim();
        if trimmed.is_empty() {
            return Ok(CommandOutcome::Agent(AgentAction::ToolsToggle(None)));
        }
        let mut parts = trimmed.split_whitespace();
        let verb = parts.next().unwrap_or("").to_lowercase();
        let names: Vec<String> = parts.map(|s| s.to_string()).collect();
        match verb.as_str() {
            "enable" => {
                if names.is_empty() {
                    return Ok(CommandOutcome::Status(
                        "usage: /tools enable <name> [name…]".into(),
                    ));
                }
                Ok(CommandOutcome::Agent(AgentAction::ToolsToggle(Some((
                    true, names,
                )))))
            }
            "disable" => {
                if names.is_empty() {
                    return Ok(CommandOutcome::Status(
                        "usage: /tools disable <name> [name…]".into(),
                    ));
                }
                Ok(CommandOutcome::Agent(AgentAction::ToolsToggle(Some((
                    false, names,
                )))))
            }
            "list" | "" => Ok(CommandOutcome::Agent(AgentAction::ToolsToggle(None))),
            other => Ok(CommandOutcome::Status(format!(
                "/tools: unknown verb '{other}' (expected list/enable/disable)"
            ))),
        }
    }
}

struct Reload;
impl CommandHandler for Reload {
    fn name(&self) -> &'static str {
        "reload"
    }
    fn help(&self) -> &'static str {
        "re-read ~/.fennec/.env into the running process"
    }
    fn execute(&self, _args: &str, _app: &mut App) -> Result<CommandOutcome> {
        Ok(CommandOutcome::Agent(AgentAction::ReloadEnv))
    }
}

struct ReloadMcp;
impl CommandHandler for ReloadMcp {
    fn name(&self) -> &'static str {
        "reload-mcp"
    }
    fn help(&self) -> &'static str {
        "rescan MCP servers in the live session"
    }
    fn aliases(&self) -> &[&'static str] {
        &["reload_mcp"]
    }
    fn execute(&self, _args: &str, _app: &mut App) -> Result<CommandOutcome> {
        Ok(CommandOutcome::Agent(AgentAction::ReloadMcp))
    }
}

// -- Clipboard / image ------------------------------------------

struct Image;
impl CommandHandler for Image {
    fn name(&self) -> &'static str {
        "image"
    }
    fn help(&self) -> &'static str {
        "attach an image to the next message"
    }
    fn execute(&self, args: &str, _app: &mut App) -> Result<CommandOutcome> {
        let path = args.trim();
        if path.is_empty() {
            return Ok(CommandOutcome::Status(
                "usage: /image <path>".into(),
            ));
        }
        // Expand a leading `~` so users typing `/image ~/Pictures/foo.png`
        // hit the right file. Anything else is passed through verbatim.
        let expanded = if let Some(rest) = path.strip_prefix("~/") {
            dirs::home_dir()
                .map(|h| h.join(rest))
                .unwrap_or_else(|| std::path::PathBuf::from(path))
        } else {
            std::path::PathBuf::from(path)
        };
        Ok(CommandOutcome::Agent(AgentAction::AttachImage(expanded)))
    }
}

struct Paste;
impl CommandHandler for Paste {
    fn name(&self) -> &'static str {
        "paste"
    }
    fn help(&self) -> &'static str {
        "attach a clipboard image to the next message"
    }
    fn execute(&self, args: &str, _app: &mut App) -> Result<CommandOutcome> {
        if !args.trim().is_empty() {
            return Ok(CommandOutcome::Status("usage: /paste".into()));
        }
        Ok(CommandOutcome::Agent(AgentAction::PasteClipboardImage))
    }
}

struct Copy;
impl CommandHandler for Copy {
    fn name(&self) -> &'static str {
        "copy"
    }
    fn help(&self) -> &'static str {
        "copy an assistant message to the OS clipboard"
    }
    fn execute(&self, args: &str, _app: &mut App) -> Result<CommandOutcome> {
        let trimmed = args.trim();
        let n = if trimmed.is_empty() {
            None
        } else {
            match trimmed.parse::<usize>() {
                Ok(v) if v >= 1 => Some(v),
                _ => {
                    return Ok(CommandOutcome::Status(
                        "usage: /copy [n] — n is 1-indexed".into(),
                    ));
                }
            }
        };
        Ok(CommandOutcome::Agent(AgentAction::CopyAssistantMessage(n)))
    }
}

struct Agents;
impl CommandHandler for Agents {
    fn name(&self) -> &'static str {
        "agents"
    }
    fn help(&self) -> &'static str {
        "open spawn-tree dashboard (or /agents pause|resume|status)"
    }
    fn aliases(&self) -> &[&'static str] {
        &["tasks"]
    }
    fn execute(&self, args: &str, app: &mut App) -> Result<CommandOutcome> {
        let trimmed = args.trim().to_lowercase();
        match trimmed.as_str() {
            "" | "open" | "show" => {
                app.show_agents_overlay = true;
                // Default the cursor to the first root if empty.
                if app.agents_cursor.is_none() {
                    if let Some(first) = app.spawn_tree.root_ids.first() {
                        app.agents_cursor = Some(first.clone());
                    } else if let Some(snap) = app.spawn_history.get(0) {
                        if let Some(first) = snap.tree.root_ids.first() {
                            app.agents_cursor = Some(first.clone());
                        }
                    }
                }
                Ok(CommandOutcome::Status("agents overlay open".into()))
            }
            "close" | "hide" => {
                app.show_agents_overlay = false;
                Ok(CommandOutcome::Status("agents overlay closed".into()))
            }
            "pause" | "resume" => {
                let target_paused = trimmed == "pause";
                match app.delegation_registry.as_ref() {
                    Some(reg) => {
                        reg.set_paused(target_paused);
                        let line = if target_paused {
                            "delegation · paused"
                        } else {
                            "delegation · resumed"
                        };
                        push_system(app, line.to_string());
                        Ok(CommandOutcome::Status(line.into()))
                    }
                    None => {
                        push_system(
                            app,
                            "/agents pause: delegation registry not attached (TUI mode only)".into(),
                        );
                        Ok(CommandOutcome::Status("no registry".into()))
                    }
                }
            }
            "status" => {
                match app.delegation_registry.as_ref() {
                    Some(reg) => {
                        let (paused, caps) = reg.status();
                        let active = reg.active_snapshot();
                        let line = format!(
                            "delegation · {} · caps d{}/{} · {} active",
                            if paused { "paused" } else { "active" },
                            caps.max_spawn_depth,
                            caps.max_concurrent_children,
                            active.len(),
                        );
                        push_system(app, line.clone());
                        Ok(CommandOutcome::Status(line))
                    }
                    None => {
                        push_system(
                            app,
                            "/agents status: delegation registry not attached (TUI mode only)".into(),
                        );
                        Ok(CommandOutcome::Status("no registry".into()))
                    }
                }
            }
            other => Ok(CommandOutcome::Status(format!(
                "/agents: unknown subcommand '{other}' (expected pause/resume/status or no arg)"
            ))),
        }
    }
}

// -- F1-2-E command set -----------------------------------------

struct Fortune;
impl CommandHandler for Fortune {
    fn name(&self) -> &'static str {
        "fortune"
    }
    fn help(&self) -> &'static str {
        "show a random or daily fortune"
    }
    fn execute(&self, args: &str, app: &mut App) -> Result<CommandOutcome> {
        let mode = args.trim().to_lowercase();
        let line = match mode.as_str() {
            "" | "random" => random_fortune(),
            "daily" | "stable" | "today" => daily_fortune(
                app.current_session_id.as_deref().unwrap_or("anon"),
            ),
            _ => {
                return Ok(CommandOutcome::Status(
                    "usage: /fortune [random|daily]".into(),
                ));
            }
        };
        push_system(app, line);
        Ok(CommandOutcome::Status("ok".into()))
    }
}

const FORTUNES: &[&str] = &[
    "🦊 minimal diff, maximal calm",
    "🌅 the desert remembers every footprint",
    "✨ ship the smaller change first",
    "🔥 a clean compile is its own reward",
    "🌾 test what you trust; trust what you test",
    "🪶 brevity is the soul of code review",
    "🌙 sleep on the breaking change",
    "🧭 every refactor is an act of optimism",
    "🍶 read the error before fixing the symptom",
    "🌵 patience scales; haste compounds",
];
const LEGENDARIES: &[&str] = &[
    "🌟 legendary drop: the fox's silent compile",
    "🌟 legendary drop: the merge that fixed three bugs you didn't know about",
    "🌟 legendary drop: a green CI run on Monday morning",
];

fn fortune_hash(seed: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    seed.hash(&mut h);
    h.finish()
}

fn random_fortune() -> String {
    // Use a fresh-each-call seed via nanos so /fortune feels live.
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    if n % 20 == 0 {
        LEGENDARIES[(n as usize) % LEGENDARIES.len()].to_string()
    } else {
        FORTUNES[(n as usize) % FORTUNES.len()].to_string()
    }
}

fn daily_fortune(session_id: &str) -> String {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let seed = format!("{session_id}|{today}");
    let n = fortune_hash(&seed);
    if n % 20 == 0 {
        LEGENDARIES[(n as usize) % LEGENDARIES.len()].to_string()
    } else {
        FORTUNES[(n as usize) % FORTUNES.len()].to_string()
    }
}

struct Queue;
impl CommandHandler for Queue {
    fn name(&self) -> &'static str {
        "queue"
    }
    fn help(&self) -> &'static str {
        "enqueue a message for the next turn (or count queued)"
    }
    fn execute(&self, args: &str, app: &mut App) -> Result<CommandOutcome> {
        let text = args.trim();
        if text.is_empty() {
            let n = app.queued_input.len();
            let body = if n == 0 {
                "queue is empty".to_string()
            } else {
                format!(
                    "{n} queued message{}",
                    if n == 1 { "" } else { "s" }
                )
            };
            push_system(app, body);
            return Ok(CommandOutcome::Status("ok".into()));
        }
        app.queued_input.push_back(text.to_string());
        let preview: String = text.chars().take(50).collect();
        let suffix = if text.chars().count() > 50 { "…" } else { "" };
        push_system(app, format!("queued: \"{preview}{suffix}\""));
        Ok(CommandOutcome::Status("queued".into()))
    }
}

struct StatusBar;
impl CommandHandler for StatusBar {
    fn name(&self) -> &'static str {
        "statusbar"
    }
    fn help(&self) -> &'static str {
        "status bar position (top|bottom|off|toggle)"
    }
    fn aliases(&self) -> &[&'static str] {
        &["sb"]
    }
    fn execute(&self, args: &str, app: &mut App) -> Result<CommandOutcome> {
        let raw = args.trim().to_lowercase();
        let next = match raw.as_str() {
            "" | "toggle" => app.statusbar_position.toggle(),
            other => match crate::tui::app::StatusBarPosition::parse(other) {
                Some(p) => p,
                None => {
                    return Ok(CommandOutcome::Status(
                        "usage: /statusbar [top|bottom|off|toggle]".into(),
                    ));
                }
            },
        };
        if next == app.statusbar_position {
            push_system(
                app,
                format!("status bar already {}", next.as_str()),
            );
            return Ok(CommandOutcome::Status("noop".into()));
        }
        app.statusbar_position = next;
        push_system(app, format!("status bar {}", next.as_str()));
        Ok(CommandOutcome::Agent(AgentAction::PersistTuiSettings))
    }
}

struct Logs;
impl CommandHandler for Logs {
    fn name(&self) -> &'static str {
        "logs"
    }
    fn help(&self) -> &'static str {
        "show the last n tracing lines (1..80, default 20)"
    }
    fn execute(&self, args: &str, app: &mut App) -> Result<CommandOutcome> {
        let n_raw = args.trim();
        let n: usize = if n_raw.is_empty() {
            20
        } else {
            n_raw.parse::<usize>().unwrap_or(20).clamp(1, 80)
        };
        let lines = app.log_ring.tail(n);
        if lines.is_empty() {
            push_system(app, "no tracing output captured yet".into());
            return Ok(CommandOutcome::Status("empty".into()));
        }
        let mut body = format!("last {} log line{}:\n", lines.len(), if lines.len() == 1 { "" } else { "s" });
        for line in &lines {
            body.push_str("  ");
            body.push_str(line);
            body.push('\n');
        }
        push_system(app, body.trim_end().to_string());
        Ok(CommandOutcome::Status(format!("shown {}", lines.len())))
    }
}

struct Indicator;
impl CommandHandler for Indicator {
    fn name(&self) -> &'static str {
        "indicator"
    }
    fn help(&self) -> &'static str {
        "spinner style (braille|ascii|kaomoji|emoji|unicode)"
    }
    fn execute(&self, args: &str, app: &mut App) -> Result<CommandOutcome> {
        let raw = args.trim().to_lowercase();
        if raw.is_empty() {
            push_system(
                app,
                format!("indicator: {}", app.indicator_style.as_str()),
            );
            return Ok(CommandOutcome::Status("ok".into()));
        }
        let Some(next) = crate::tui::app::IndicatorStyle::parse(&raw) else {
            return Ok(CommandOutcome::Status(
                "usage: /indicator [braille|ascii|kaomoji|emoji|unicode]".into(),
            ));
        };
        if next == app.indicator_style {
            push_system(app, format!("indicator already {}", next.as_str()));
            return Ok(CommandOutcome::Status("noop".into()));
        }
        app.indicator_style = next;
        push_system(app, format!("indicator → {}", next.as_str()));
        Ok(CommandOutcome::Agent(AgentAction::PersistTuiSettings))
    }
}

struct ReloadSkills;
impl CommandHandler for ReloadSkills {
    fn name(&self) -> &'static str {
        "reload-skills"
    }
    fn help(&self) -> &'static str {
        "rescan ~/.fennec/skills and refresh the agent's prompt"
    }
    fn aliases(&self) -> &[&'static str] {
        &["reload_skills"]
    }
    fn execute(&self, _args: &str, _app: &mut App) -> Result<CommandOutcome> {
        Ok(CommandOutcome::Agent(AgentAction::ReloadSkills))
    }
}

struct Verbose;
impl CommandHandler for Verbose {
    fn name(&self) -> &'static str {
        "verbose"
    }
    fn help(&self) -> &'static str {
        "tool-output verbosity (on|off|cycle)"
    }
    fn execute(&self, args: &str, app: &mut App) -> Result<CommandOutcome> {
        let raw = args.trim().to_lowercase();
        let next = match raw.as_str() {
            "" | "cycle" => app.verbosity.toggle(),
            other => match crate::tui::app::VerbosityMode::parse(other) {
                Some(v) => v,
                None => {
                    return Ok(CommandOutcome::Status(
                        "usage: /verbose [on|off|cycle]".into(),
                    ));
                }
            },
        };
        if next == app.verbosity {
            push_system(app, format!("verbose already {}", next.as_str()));
            return Ok(CommandOutcome::Status("noop".into()));
        }
        app.verbosity = next;
        push_system(app, format!("verbose: {}", next.as_str()));
        Ok(CommandOutcome::Agent(AgentAction::PersistTuiSettings))
    }
}

struct Busy;
impl CommandHandler for Busy {
    fn name(&self) -> &'static str {
        "busy"
    }
    fn help(&self) -> &'static str {
        "Enter mid-turn behaviour (interrupt|queue|steer|status)"
    }
    fn execute(&self, args: &str, app: &mut App) -> Result<CommandOutcome> {
        let raw = args.trim().to_lowercase();
        match raw.as_str() {
            "" | "status" => {
                push_system(
                    app,
                    format!("busy input mode: {}", app.busy_mode.as_str()),
                );
                Ok(CommandOutcome::Status("ok".into()))
            }
            other => match crate::tui::app::BusyMode::parse(other) {
                Some(next) => {
                    if next == app.busy_mode {
                        push_system(
                            app,
                            format!("busy already {}", next.as_str()),
                        );
                        return Ok(CommandOutcome::Status("noop".into()));
                    }
                    app.busy_mode = next;
                    push_system(app, format!("busy → {}", next.as_str()));
                    Ok(CommandOutcome::Agent(AgentAction::PersistTuiSettings))
                }
                None => Ok(CommandOutcome::Status(
                    "usage: /busy [interrupt|queue|steer|status]".into(),
                )),
            },
        }
    }
}

struct Reasoning;
impl CommandHandler for Reasoning {
    fn name(&self) -> &'static str {
        "reasoning"
    }
    fn help(&self) -> &'static str {
        "thinking effort + visibility (off|low|medium|high|xhigh|hide|show)"
    }
    fn execute(&self, args: &str, app: &mut App) -> Result<CommandOutcome> {
        use crate::agent::thinking::ThinkingLevel;
        let raw = args.trim().to_lowercase();
        match raw.as_str() {
            "" => {
                push_system(app, "usage: /reasoning [off|low|medium|high|xhigh|hide|show]".into());
                Ok(CommandOutcome::Status("ok".into()))
            }
            "hide" => {
                app.show_reasoning = false;
                push_system(app, "reasoning · hidden".into());
                Ok(CommandOutcome::Agent(AgentAction::PersistTuiSettings))
            }
            "show" => {
                app.show_reasoning = true;
                push_system(app, "reasoning · shown".into());
                Ok(CommandOutcome::Agent(AgentAction::PersistTuiSettings))
            }
            "off" | "low" | "medium" | "high" | "max" | "xhigh" => {
                let level = match raw.as_str() {
                    "off" => ThinkingLevel::Off,
                    "low" => ThinkingLevel::Low,
                    "medium" => ThinkingLevel::Medium,
                    "high" => ThinkingLevel::High,
                    "max" | "xhigh" => ThinkingLevel::Max,
                    _ => unreachable!(),
                };
                push_system(app, format!("reasoning effort → {raw}"));
                Ok(CommandOutcome::Agent(AgentAction::SetThinkingLevel(level)))
            }
            _ => Ok(CommandOutcome::Status(
                "usage: /reasoning [off|low|medium|high|max|hide|show]".into(),
            )),
        }
    }
}

struct Personality;
impl CommandHandler for Personality {
    fn name(&self) -> &'static str {
        "personality"
    }
    fn help(&self) -> &'static str {
        "swap the agent's persona by preset name (empty = reset)"
    }
    fn execute(&self, args: &str, app: &mut App) -> Result<CommandOutcome> {
        let name = args.trim().to_string();
        if name == app.personality_name {
            let label = if name.is_empty() { "default".into() } else { name.clone() };
            push_system(app, format!("personality already {label}"));
            return Ok(CommandOutcome::Status("noop".into()));
        }
        let preset_persona = lookup_personality(&name);
        match preset_persona {
            Some(persona) => {
                app.personality_name = name.clone();
                let label = if name.is_empty() { "default".into() } else { name };
                push_system(app, format!("personality → {label}"));
                Ok(CommandOutcome::Agent(AgentAction::SetPersona(persona)))
            }
            None => Ok(CommandOutcome::Status(format!(
                "/personality: unknown preset '{name}' (see /help for the list)"
            ))),
        }
    }
}

/// Built-in personality presets. Map a name → persona string the
/// agent injects into its system prompt. Empty `""` name resets
/// to the IdentityConfig default. Users can add their own presets
/// later via config; built-ins ship out of the box.
fn lookup_personality(name: &str) -> Option<String> {
    match name {
        "" | "default" => Some(
            "Your personal AI agent — sharp, resourceful, and always on.".to_string(),
        ),
        "terse" => Some(
            "A focused operator. Brief replies, no preamble, never reasoning out loud unless asked.".to_string(),
        ),
        "tutor" => Some(
            "A patient teacher. Explains every step, anticipates confusion, offers concrete examples.".to_string(),
        ),
        "reviewer" => Some(
            "A meticulous reviewer. Calls out edge cases, security issues, naming, and consistency. \
             Pushes back on ambiguous requirements.".to_string(),
        ),
        "researcher" => Some(
            "A curious investigator. Pulls primary sources, cross-checks claims, distinguishes data from opinion.".to_string(),
        ),
        _ => None,
    }
}

struct Branch;
impl CommandHandler for Branch {
    fn name(&self) -> &'static str {
        "branch"
    }
    fn help(&self) -> &'static str {
        "fork the current session into a fresh row"
    }
    fn aliases(&self) -> &[&'static str] {
        &["fork"]
    }
    fn execute(&self, args: &str, _app: &mut App) -> Result<CommandOutcome> {
        let raw = args.trim();
        let title = if raw.is_empty() {
            None
        } else {
            Some(raw.to_string())
        };
        Ok(CommandOutcome::Agent(AgentAction::BranchSession(title)))
    }
}

struct Replay;
impl CommandHandler for Replay {
    fn name(&self) -> &'static str {
        "replay"
    }
    fn help(&self) -> &'static str {
        "open a completed spawn tree (N|last|list)"
    }
    fn execute(&self, args: &str, app: &mut App) -> Result<CommandOutcome> {
        let raw = args.trim();
        let lower = raw.to_lowercase();
        if app.spawn_history.is_empty() {
            push_system(app, "no completed spawn trees this session".into());
            return Ok(CommandOutcome::Status("empty".into()));
        }
        if lower == "list" || lower == "ls" {
            let mut body = format!(
                "spawn history · {} snapshot{}:\n",
                app.spawn_history.len(),
                if app.spawn_history.len() == 1 { "" } else { "s" }
            );
            for i in 0..app.spawn_history.len() {
                if let Some(snap) = app.spawn_history.get(i) {
                    body.push_str(&format!(
                        "  {idx}. {n} agent{plural} · {label}\n",
                        idx = i + 1,
                        n = snap.tree.len(),
                        plural = if snap.tree.len() == 1 { "" } else { "s" },
                        label = snap.label,
                    ));
                }
            }
            push_system(app, body.trim_end().to_string());
            return Ok(CommandOutcome::Status("listed".into()));
        }
        let max = app.spawn_history.len();
        let index = if raw.is_empty() || lower == "last" {
            1
        } else {
            match raw.parse::<usize>() {
                Ok(n) if n >= 1 && n <= max => n,
                _ => {
                    return Ok(CommandOutcome::Status(format!(
                        "/replay: index out of range 1..{max}"
                    )));
                }
            }
        };
        app.agents_history_index = index;
        app.show_agents_overlay = true;
        app.agents_cursor = app.agents_flat_node_ids().first().cloned();
        push_system(app, format!("replay · {index}/{max}"));
        Ok(CommandOutcome::Status("ok".into()))
    }
}

struct ReplayDiff;
impl CommandHandler for ReplayDiff {
    fn name(&self) -> &'static str {
        "replay-diff"
    }
    fn help(&self) -> &'static str {
        "compare two spawn-tree snapshots (history indexes a b)"
    }
    fn execute(&self, args: &str, app: &mut App) -> Result<CommandOutcome> {
        let parts: Vec<&str> = args.split_whitespace().collect();
        if parts.len() != 2 {
            return Ok(CommandOutcome::Status(
                "usage: /replay-diff <a> <b>  (e.g. /replay-diff 1 2 for last two)".into(),
            ));
        }
        let parse_idx = |s: &str| -> Option<usize> {
            s.parse::<usize>()
                .ok()
                .filter(|n| *n >= 1 && *n <= app.spawn_history.len())
        };
        let (Some(a), Some(b)) = (parse_idx(parts[0]), parse_idx(parts[1])) else {
            return Ok(CommandOutcome::Status(format!(
                "/replay-diff: could not resolve indices · history has {} entries",
                app.spawn_history.len()
            )));
        };
        app.agents_diff_pair = Some((a, b));
        app.agents_history_index = 0;
        app.show_agents_overlay = true;
        push_system(app, format!("diff · {a} → {b}"));
        Ok(CommandOutcome::Status("ok".into()))
    }
}

// -- Helpers ----------------------------------------------------

fn push_system(app: &mut App, body: String) {
    app.chat.push(ChatLine::System {
        time: chrono::Local::now().format("%H:%M:%S").to_string(),
        body,
    });
}

/// Parse `on|off|toggle` (default = toggle) and return the
/// resulting bool.
fn parse_toggle(args: &str, current: bool) -> bool {
    match args.trim().to_lowercase().as_str() {
        "on" | "true" | "1" | "yes" => true,
        "off" | "false" | "0" | "no" => false,
        _ => !current,
    }
}

fn first_line(s: &str, max: usize) -> String {
    let one = s.lines().next().unwrap_or("").to_string();
    if one.chars().count() <= max {
        one
    } else {
        let mut out: String = one.chars().take(max - 1).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{BusyMode, IndicatorStyle, StatusBarPosition, VerbosityMode};

    fn dispatch(name: &str, args: &str) -> (CommandOutcome, App) {
        let r = CommandRegistry::with_builtins();
        let mut app = App::new();
        let outcome = r.dispatch(name, args, &mut app).unwrap();
        (outcome, app)
    }

    #[test]
    fn help_appends_command_list() {
        let (_outcome, app) = dispatch("help", "");
        let last = app.chat.last().unwrap();
        let text = match last {
            ChatLine::System { body, .. } => body.clone(),
            _ => panic!("expected system line"),
        };
        assert!(text.contains("/help"));
        assert!(text.contains("/quit"));
        assert!(text.contains("/clear"));
    }

    #[test]
    fn quit_sets_should_quit() {
        let (outcome, app) = dispatch("quit", "");
        assert!(matches!(outcome, CommandOutcome::Quit));
        assert!(app.should_quit);
    }

    #[test]
    fn quit_alias_q() {
        let (outcome, app) = dispatch("q", "");
        assert!(matches!(outcome, CommandOutcome::Quit));
        assert!(app.should_quit);
    }

    #[test]
    fn clear_clears_chat_and_returns_agent_action() {
        let r = CommandRegistry::with_builtins();
        let mut app = App::new();
        app.chat.push(ChatLine::User {
            time: "00:00:00".into(),
            body: "hi".into(),
        });
        let outcome = r.dispatch("clear", "", &mut app).unwrap();
        assert!(matches!(outcome, CommandOutcome::Agent(AgentAction::Clear)));
        assert!(app.chat.is_empty());
    }

    #[test]
    fn new_aliases_clear() {
        let (outcome, app) = dispatch("new", "");
        assert!(matches!(outcome, CommandOutcome::Agent(AgentAction::Clear)));
        assert!(app.chat.is_empty());
    }

    #[test]
    fn compact_toggles_app_flag() {
        let r = CommandRegistry::with_builtins();
        let mut app = App::new();
        assert!(!app.compact_mode);
        r.dispatch("compact", "", &mut app).unwrap();
        assert!(app.compact_mode);
        r.dispatch("compact", "", &mut app).unwrap();
        assert!(!app.compact_mode);
        r.dispatch("compact", "on", &mut app).unwrap();
        assert!(app.compact_mode);
        r.dispatch("compact", "off", &mut app).unwrap();
        assert!(!app.compact_mode);
    }

    #[test]
    fn details_no_arg_cycles_through_modes() {
        use crate::tui::app::DetailsMode;
        let r = CommandRegistry::with_builtins();
        let mut app = App::new();
        // Default is Expanded.
        assert_eq!(app.details_mode, DetailsMode::Expanded);
        r.dispatch("details", "", &mut app).unwrap();
        assert_eq!(app.details_mode, DetailsMode::Collapsed);
        r.dispatch("details", "", &mut app).unwrap();
        assert_eq!(app.details_mode, DetailsMode::Hidden);
        r.dispatch("details", "", &mut app).unwrap();
        assert_eq!(app.details_mode, DetailsMode::Expanded);
    }

    #[test]
    fn details_with_arg_sets_explicit_mode() {
        use crate::tui::app::DetailsMode;
        let r = CommandRegistry::with_builtins();
        let mut app = App::new();
        r.dispatch("details", "hidden", &mut app).unwrap();
        assert_eq!(app.details_mode, DetailsMode::Hidden);
        r.dispatch("details", "expanded", &mut app).unwrap();
        assert_eq!(app.details_mode, DetailsMode::Expanded);
    }

    #[test]
    fn details_with_unknown_arg_returns_status_and_does_not_change_state() {
        use crate::tui::app::DetailsMode;
        let r = CommandRegistry::with_builtins();
        let mut app = App::new();
        let initial = app.details_mode;
        let outcome = r.dispatch("details", "loud", &mut app).unwrap();
        match outcome {
            CommandOutcome::Status(s) => {
                assert!(s.contains("unknown mode"), "got: {s}")
            }
            other => panic!("expected Status, got {:?}", other),
        }
        // Untouched on parse failure.
        assert_eq!(app.details_mode, initial);
        assert_eq!(initial, DetailsMode::Expanded);
    }

    #[test]
    fn agents_no_arg_opens_overlay() {
        let r = CommandRegistry::with_builtins();
        let mut app = App::new();
        assert!(!app.show_agents_overlay);
        r.dispatch("agents", "", &mut app).unwrap();
        assert!(app.show_agents_overlay);
    }

    #[test]
    fn agents_close_subcommand_closes_overlay() {
        let r = CommandRegistry::with_builtins();
        let mut app = App::new();
        app.show_agents_overlay = true;
        r.dispatch("agents", "close", &mut app).unwrap();
        assert!(!app.show_agents_overlay);
    }

    #[test]
    fn agents_pause_without_registry_explains_tui_only() {
        let r = CommandRegistry::with_builtins();
        let mut app = App::new();
        assert!(app.delegation_registry.is_none());
        r.dispatch("agents", "pause", &mut app).unwrap();
        let body = app
            .chat
            .iter()
            .rev()
            .find_map(|l| match l {
                ChatLine::System { body, .. } => Some(body.clone()),
                _ => None,
            })
            .unwrap();
        assert!(body.contains("not attached"), "got: {body}");
    }

    #[test]
    fn agents_pause_with_registry_flips_pause_flag() {
        let r = CommandRegistry::with_builtins();
        let mut app = App::new();
        let reg = crate::agent::DelegationRegistry::default();
        app.delegation_registry = Some(reg.clone());
        assert!(!reg.is_paused());
        r.dispatch("agents", "pause", &mut app).unwrap();
        assert!(reg.is_paused());
        r.dispatch("agents", "resume", &mut app).unwrap();
        assert!(!reg.is_paused());
    }

    #[test]
    fn agents_status_with_registry_renders_one_liner() {
        let r = CommandRegistry::with_builtins();
        let mut app = App::new();
        app.delegation_registry = Some(crate::agent::DelegationRegistry::default());
        r.dispatch("agents", "status", &mut app).unwrap();
        let body = app
            .chat
            .iter()
            .rev()
            .find_map(|l| match l {
                ChatLine::System { body, .. } => Some(body.clone()),
                _ => None,
            })
            .unwrap();
        assert!(body.contains("delegation · active · caps d"), "got: {body}");
        assert!(body.contains("0 active"), "got: {body}");
    }

    #[test]
    fn agents_alias_tasks_also_works() {
        let r = CommandRegistry::with_builtins();
        let mut app = App::new();
        r.dispatch("tasks", "", &mut app).unwrap();
        assert!(app.show_agents_overlay);
    }

    #[test]
    fn details_emits_persist_action_on_change() {
        let (outcome, _app) = dispatch("details", "collapsed");
        match outcome {
            CommandOutcome::Agent(AgentAction::PersistTuiSettings) => {}
            other => panic!("expected PersistTuiSettings, got {:?}", other),
        }
    }

    #[test]
    fn compact_emits_persist_action() {
        // The submit loop saves config.toml when /compact fires
        // — we verify the outcome shape so a regression that
        // drops the persistence side gets caught here rather
        // than as silent data loss across restarts.
        let (outcome, _app) = dispatch("compact", "on");
        match outcome {
            CommandOutcome::Agent(AgentAction::PersistTuiSettings) => {}
            other => panic!("expected PersistTuiSettings, got {:?}", other),
        }
    }

    #[test]
    fn steer_requires_arg() {
        let (outcome, _app) = dispatch("steer", "");
        match outcome {
            CommandOutcome::Status(s) => assert!(s.contains("missing")),
            other => panic!("expected Status, got {:?}", other),
        }
    }

    #[test]
    fn steer_with_arg_returns_agent_action() {
        let (outcome, _app) = dispatch("steer", "use markdown");
        match outcome {
            CommandOutcome::Agent(AgentAction::Steer(s)) => {
                assert_eq!(s, "use markdown");
            }
            other => panic!("expected Steer action, got {:?}", other),
        }
    }

    fn dispatch_with_skills_dir(
        name: &str,
        args: &str,
        skills_dir: &std::path::Path,
    ) -> (CommandOutcome, App) {
        let r = CommandRegistry::with_builtins();
        let mut app = App::new();
        app.skills_dir = Some(skills_dir.to_path_buf());
        let outcome = r.dispatch(name, args, &mut app).unwrap();
        (outcome, app)
    }

    fn write_skill(dir: &std::path::Path, name: &str, body: &str) {
        std::fs::write(dir.join(format!("{name}.md")), body).unwrap();
    }

    fn skill_md(name: &str, description: &str) -> String {
        format!(
            "---\nname: {name}\ndescription: {description}\nalways: false\n---\n# {name}\n\nbody for {name}\n"
        )
    }

    #[test]
    fn skills_list_with_no_dir_returns_status() {
        let (outcome, _app) = dispatch("skills", "");
        match outcome {
            CommandOutcome::Status(s) => assert!(s.contains("not configured"), "got: {s}"),
            other => panic!("expected Status, got {:?}", other),
        }
    }

    #[test]
    fn skills_list_renders_loaded_skills() {
        let dir = tempfile::TempDir::new().unwrap();
        write_skill(dir.path(), "alpha", &skill_md("alpha", "alpha description"));
        write_skill(dir.path(), "beta", &skill_md("beta", "beta description"));
        let (_o, app) = dispatch_with_skills_dir("skills", "", dir.path());
        let body = app
            .chat
            .iter()
            .rev()
            .find_map(|l| match l {
                ChatLine::System { body, .. } => Some(body.clone()),
                _ => None,
            })
            .expect("expected a system line");
        assert!(body.contains("alpha"), "{body}");
        assert!(body.contains("beta"), "{body}");
        assert!(body.contains("alpha description"), "{body}");
    }

    #[test]
    fn skills_search_filters_by_substring() {
        let dir = tempfile::TempDir::new().unwrap();
        write_skill(dir.path(), "git-commit", &skill_md("git-commit", "Commit hygiene helpers"));
        write_skill(dir.path(), "weather", &skill_md("weather", "Forecast lookup"));
        let (_o, app) = dispatch_with_skills_dir("skills", "search hygiene", dir.path());
        let body = app
            .chat
            .iter()
            .rev()
            .find_map(|l| match l {
                ChatLine::System { body, .. } => Some(body.clone()),
                _ => None,
            })
            .unwrap();
        assert!(body.contains("git-commit"), "{body}");
        assert!(!body.contains("weather"), "{body}");
    }

    #[test]
    fn skills_inspect_shows_full_details() {
        let dir = tempfile::TempDir::new().unwrap();
        write_skill(
            dir.path(),
            "deploy",
            &skill_md("deploy", "Deployment runbook"),
        );
        let (_o, app) = dispatch_with_skills_dir("skills", "inspect deploy", dir.path());
        let body = app
            .chat
            .iter()
            .rev()
            .find_map(|l| match l {
                ChatLine::System { body, .. } => Some(body.clone()),
                _ => None,
            })
            .unwrap();
        assert!(body.starts_with("Skill: deploy"), "{body}");
        assert!(body.contains("Deployment runbook"), "{body}");
        assert!(body.contains("body for deploy"), "{body}");
    }

    #[test]
    fn skills_install_points_at_skills_hub_pr() {
        let dir = tempfile::TempDir::new().unwrap();
        let (_o, app) = dispatch_with_skills_dir("skills", "install foo", dir.path());
        let body = app
            .chat
            .iter()
            .rev()
            .find_map(|l| match l {
                ChatLine::System { body, .. } => Some(body.clone()),
                _ => None,
            })
            .unwrap();
        assert!(body.contains("skills hub"), "{body}");
    }

    #[test]
    fn copy_with_no_arg_emits_copy_action_for_last_message() {
        // The handler delegates to the submit loop via
        // AgentAction::CopyAssistantMessage. Empty-chat handling
        // (the "nothing to copy" path) lives in main.rs's
        // handle_copy_assistant, not the command handler.
        let (outcome, _app) = dispatch("copy", "");
        match outcome {
            CommandOutcome::Agent(AgentAction::CopyAssistantMessage(None)) => {}
            other => panic!("expected CopyAssistantMessage(None), got {:?}", other),
        }
    }

    #[test]
    fn copy_with_numeric_arg_passes_index_through() {
        let (outcome, _app) = dispatch("copy", "3");
        match outcome {
            CommandOutcome::Agent(AgentAction::CopyAssistantMessage(Some(3))) => {}
            other => panic!("expected CopyAssistantMessage(Some(3)), got {:?}", other),
        }
    }

    #[test]
    fn copy_with_non_numeric_arg_returns_status_usage() {
        let (outcome, _app) = dispatch("copy", "garbage");
        match outcome {
            CommandOutcome::Status(s) => assert!(s.contains("usage"), "got: {s}"),
            other => panic!("expected Status, got {:?}", other),
        }
    }

    #[test]
    fn registry_with_builtins_has_all_25() {
        let r = CommandRegistry::with_builtins();
        let names = r.names();
        // 25 unique primary commands per the F1-1 spec.
        assert!(
            names.len() >= 25,
            "expected at least 25 commands, got {}: {:?}",
            names.len(),
            names
        );
        for required in &[
            "help", "quit", "clear", "resume", "title", "save", "history",
            "compact", "details", "redraw", "mouse", "model", "voice",
            "usage", "status", "steer", "undo", "retry", "skills", "tools",
            "reload", "reload-mcp", "image", "paste", "copy", "agents",
            "fortune", "queue", "statusbar", "logs", "indicator",
            "reload-skills", "verbose", "busy", "reasoning", "personality",
            "branch", "replay", "replay-diff",
        ] {
            assert!(
                names.contains(required),
                "missing required command: {required}"
            );
        }
    }

    #[test]
    fn fortune_random_emits_system_line() {
        let r = CommandRegistry::with_builtins();
        let mut app = App::new();
        r.dispatch("fortune", "", &mut app).unwrap();
        let body = last_system_body(&app).unwrap();
        assert!(!body.is_empty());
    }

    #[test]
    fn fortune_unknown_arg_surfaces_usage() {
        let (outcome, _app) = dispatch("fortune", "weekly");
        match outcome {
            CommandOutcome::Status(s) => assert!(s.contains("usage")),
            other => panic!("expected Status with usage, got {:?}", other),
        }
    }

    #[test]
    fn queue_no_arg_reports_count() {
        let r = CommandRegistry::with_builtins();
        let mut app = App::new();
        r.dispatch("queue", "", &mut app).unwrap();
        assert!(last_system_body(&app).unwrap().contains("empty"));
        app.queued_input.push_back("first".into());
        r.dispatch("queue", "", &mut app).unwrap();
        assert!(last_system_body(&app).unwrap().contains("1 queued message"));
    }

    #[test]
    fn queue_with_arg_enqueues_message() {
        let r = CommandRegistry::with_builtins();
        let mut app = App::new();
        r.dispatch("queue", "explore option B", &mut app).unwrap();
        assert_eq!(app.queued_input.len(), 1);
        assert_eq!(app.queued_input[0], "explore option B");
    }

    #[test]
    fn statusbar_toggle_cycles_off_and_top() {
        let r = CommandRegistry::with_builtins();
        let mut app = App::new();
        assert_eq!(app.statusbar_position, StatusBarPosition::Bottom);
        r.dispatch("statusbar", "toggle", &mut app).unwrap();
        assert_eq!(app.statusbar_position, StatusBarPosition::Off);
        r.dispatch("sb", "toggle", &mut app).unwrap();
        assert_eq!(app.statusbar_position, StatusBarPosition::Top);
    }

    #[test]
    fn statusbar_top_emits_persist() {
        let (outcome, app) = dispatch("statusbar", "top");
        assert_eq!(app.statusbar_position, StatusBarPosition::Top);
        match outcome {
            CommandOutcome::Agent(AgentAction::PersistTuiSettings) => {}
            other => panic!("expected PersistTuiSettings, got {:?}", other),
        }
    }

    #[test]
    fn logs_empty_says_no_output() {
        let r = CommandRegistry::with_builtins();
        let mut app = App::new();
        r.dispatch("logs", "", &mut app).unwrap();
        assert!(last_system_body(&app).unwrap().contains("no tracing output"));
    }

    #[test]
    fn logs_clamps_n_to_range() {
        let r = CommandRegistry::with_builtins();
        let mut app = App::new();
        for i in 0..150 {
            app.log_ring.push(format!("line {i}"));
        }
        r.dispatch("logs", "200", &mut app).unwrap();
        // 200 clamps to 80 → 80 lines shown.
        let body = last_system_body(&app).unwrap();
        assert!(body.contains("last 80 log lines"), "got: {body}");
    }

    #[test]
    fn indicator_cycles_styles() {
        let r = CommandRegistry::with_builtins();
        let mut app = App::new();
        assert_eq!(app.indicator_style, IndicatorStyle::Braille);
        r.dispatch("indicator", "kaomoji", &mut app).unwrap();
        assert_eq!(app.indicator_style, IndicatorStyle::Kaomoji);
    }

    #[test]
    fn verbose_cycle_toggles() {
        let r = CommandRegistry::with_builtins();
        let mut app = App::new();
        r.dispatch("verbose", "cycle", &mut app).unwrap();
        assert_eq!(app.verbosity, VerbosityMode::Verbose);
        r.dispatch("verbose", "off", &mut app).unwrap();
        assert_eq!(app.verbosity, VerbosityMode::Normal);
    }

    #[test]
    fn busy_status_reports_current_mode() {
        let r = CommandRegistry::with_builtins();
        let mut app = App::new();
        r.dispatch("busy", "status", &mut app).unwrap();
        assert!(last_system_body(&app).unwrap().contains("interrupt"));
    }

    #[test]
    fn busy_set_emits_persist() {
        let (outcome, app) = dispatch("busy", "queue");
        assert_eq!(app.busy_mode, BusyMode::Queue);
        match outcome {
            CommandOutcome::Agent(AgentAction::PersistTuiSettings) => {}
            other => panic!("expected PersistTuiSettings, got {:?}", other),
        }
    }

    #[test]
    fn reasoning_level_emits_set_thinking_level() {
        let (outcome, _app) = dispatch("reasoning", "high");
        match outcome {
            CommandOutcome::Agent(AgentAction::SetThinkingLevel(
                crate::agent::thinking::ThinkingLevel::High,
            )) => {}
            other => panic!("expected SetThinkingLevel(High), got {:?}", other),
        }
    }

    #[test]
    fn reasoning_hide_toggles_show_flag() {
        let (_outcome, app) = dispatch("reasoning", "hide");
        assert!(!app.show_reasoning);
    }

    #[test]
    fn personality_unknown_preset_returns_status() {
        let (outcome, app) = dispatch("personality", "anarchic");
        assert_eq!(app.personality_name, "");
        match outcome {
            CommandOutcome::Status(s) => assert!(s.contains("unknown preset")),
            other => panic!("expected unknown-preset status, got {:?}", other),
        }
    }

    #[test]
    fn personality_terse_emits_set_persona() {
        let (outcome, app) = dispatch("personality", "terse");
        assert_eq!(app.personality_name, "terse");
        match outcome {
            CommandOutcome::Agent(AgentAction::SetPersona(p)) => {
                assert!(p.starts_with("A focused operator"));
            }
            other => panic!("expected SetPersona, got {:?}", other),
        }
    }

    #[test]
    fn branch_no_arg_emits_branch_session_with_none() {
        let (outcome, _app) = dispatch("branch", "");
        match outcome {
            CommandOutcome::Agent(AgentAction::BranchSession(None)) => {}
            other => panic!("expected BranchSession(None), got {:?}", other),
        }
    }

    #[test]
    fn fork_alias_dispatches_to_branch() {
        let r = CommandRegistry::with_builtins();
        let mut app = App::new();
        let outcome = r.dispatch("fork", "explore", &mut app).unwrap();
        match outcome {
            CommandOutcome::Agent(AgentAction::BranchSession(Some(t))) => {
                assert_eq!(t, "explore");
            }
            other => panic!("expected BranchSession(Some), got {:?}", other),
        }
    }

    #[test]
    fn reload_skills_emits_action() {
        let (outcome, _app) = dispatch("reload-skills", "");
        match outcome {
            CommandOutcome::Agent(AgentAction::ReloadSkills) => {}
            other => panic!("expected ReloadSkills, got {:?}", other),
        }
    }

    #[test]
    fn replay_empty_history_says_so() {
        let r = CommandRegistry::with_builtins();
        let mut app = App::new();
        r.dispatch("replay", "", &mut app).unwrap();
        assert!(last_system_body(&app).unwrap().contains("no completed"));
    }

    #[test]
    fn replay_diff_validates_argument_count() {
        let (outcome, _app) = dispatch("replay-diff", "1");
        match outcome {
            CommandOutcome::Status(s) => assert!(s.contains("usage")),
            other => panic!("expected usage status, got {:?}", other),
        }
    }

    fn last_system_body(app: &App) -> Option<String> {
        app.chat.iter().rev().find_map(|l| match l {
            ChatLine::System { body, .. } => Some(body.clone()),
            _ => None,
        })
    }
}
