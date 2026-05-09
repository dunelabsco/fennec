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
        "toggle compact transcript rendering"
    }
    fn execute(&self, args: &str, app: &mut App) -> Result<CommandOutcome> {
        let want = parse_toggle(args, app.compact_mode);
        app.compact_mode = want;
        Ok(CommandOutcome::Status(format!(
            "compact: {}",
            if want { "on" } else { "off" }
        )))
    }
}

struct Details;
impl CommandHandler for Details {
    fn name(&self) -> &'static str {
        "details"
    }
    fn help(&self) -> &'static str {
        "toggle thinking / tool detail visibility"
    }
    fn execute(&self, args: &str, app: &mut App) -> Result<CommandOutcome> {
        let want = parse_toggle(args, app.details_visible);
        app.details_visible = want;
        Ok(CommandOutcome::Status(format!(
            "details: {}",
            if want { "shown" } else { "hidden" }
        )))
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
        "browse / inspect skills"
    }
    fn execute(&self, _args: &str, app: &mut App) -> Result<CommandOutcome> {
        push_system(
            app,
            "skills browser overlay lands in F1-2. Skills are loaded at startup; check the agent's system prompt for the active set.".into(),
        );
        Ok(CommandOutcome::Status("ok".into()))
    }
}

struct Tools;
impl CommandHandler for Tools {
    fn name(&self) -> &'static str {
        "tools"
    }
    fn help(&self) -> &'static str {
        "toggle a tool"
    }
    fn execute(&self, args: &str, app: &mut App) -> Result<CommandOutcome> {
        push_system(
            app,
            format!(
                "tools enable/disable wiring lands in F1-2. Arg seen: {:?}",
                args.trim()
            ),
        );
        Ok(CommandOutcome::Status("noted".into()))
    }
}

struct Reload;
impl CommandHandler for Reload {
    fn name(&self) -> &'static str {
        "reload"
    }
    fn help(&self) -> &'static str {
        "reload .env into the running agent"
    }
    fn execute(&self, _args: &str, app: &mut App) -> Result<CommandOutcome> {
        push_system(app, ".env reload lands in F1-2.".into());
        Ok(CommandOutcome::Status("noted".into()))
    }
}

struct ReloadMcp;
impl CommandHandler for ReloadMcp {
    fn name(&self) -> &'static str {
        "reload-mcp"
    }
    fn help(&self) -> &'static str {
        "rescan MCP servers"
    }
    fn aliases(&self) -> &[&'static str] {
        &["reload_mcp"]
    }
    fn execute(&self, _args: &str, app: &mut App) -> Result<CommandOutcome> {
        push_system(app, "MCP rescan lands in F1-2.".into());
        Ok(CommandOutcome::Status("noted".into()))
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
    fn execute(&self, args: &str, app: &mut App) -> Result<CommandOutcome> {
        let path = args.trim();
        if path.is_empty() {
            push_system(app, "image needs a file path. usage: /image <path>".into());
            return Ok(CommandOutcome::Status("missing arg".into()));
        }
        push_system(
            app,
            format!(
                "image attach for {} — wiring to the vision tool's attachments path lands in F1-2.",
                path
            ),
        );
        Ok(CommandOutcome::Status("noted".into()))
    }
}

struct Paste;
impl CommandHandler for Paste {
    fn name(&self) -> &'static str {
        "paste"
    }
    fn help(&self) -> &'static str {
        "paste clipboard text into the input"
    }
    fn execute(&self, _args: &str, app: &mut App) -> Result<CommandOutcome> {
        // Clipboard access via a cross-platform crate is a real
        // dep we'd need to add — defer to F1-2 alongside the
        // image variant. For now, emit a hint.
        push_system(
            app,
            "clipboard paste lands in F1-2. Most terminals already paste with Cmd-V / Ctrl-Shift-V into the input.".into(),
        );
        Ok(CommandOutcome::Status("noted".into()))
    }
}

struct Copy;
impl CommandHandler for Copy {
    fn name(&self) -> &'static str {
        "copy"
    }
    fn help(&self) -> &'static str {
        "copy a past assistant message"
    }
    fn execute(&self, args: &str, app: &mut App) -> Result<CommandOutcome> {
        // Find last bot message, or the n-th from the end.
        let n: usize = args.trim().parse().unwrap_or(0);
        let bot_messages: Vec<&str> = app
            .chat
            .iter()
            .filter_map(|l| match l {
                ChatLine::Bot { body, .. } => Some(body.as_str()),
                _ => None,
            })
            .collect();
        if bot_messages.is_empty() {
            push_system(app, "no assistant messages to copy yet.".into());
            return Ok(CommandOutcome::Status("nothing to copy".into()));
        }
        let idx = bot_messages.len().saturating_sub(1 + n);
        let target = bot_messages[idx];
        // Real clipboard write lands in F1-2 (needs a platform
        // crate). For now, surface the text so the user can
        // select it manually.
        push_system(
            app,
            format!(
                "(clipboard write lands in F1-2) message {} of {}:\n{target}",
                idx + 1,
                bot_messages.len()
            ),
        );
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

    #[test]
    fn copy_with_no_bot_messages_status_is_nothing() {
        let (outcome, _app) = dispatch("copy", "");
        match outcome {
            CommandOutcome::Status(s) => assert!(s.contains("nothing")),
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
            "reload", "reload-mcp", "image", "paste", "copy",
        ] {
            assert!(
                names.contains(required),
                "missing required command: {required}"
            );
        }
    }
}
