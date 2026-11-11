//! The agent harnesses a role can run in (AGENT_KINDS, herdr's `--kind`), as orchestrator.py's "Agent harnesses"
//! section and its session logs know them, but for opencode's model and granted directory, which go in its config
//! (Config::OpencodeEnv): how each harness's agent starts, resumes a saved session and is granted a directory
//! ([`Harness::launch`]), and where and how it records the tokens a session used ([`SessionLog`],
//! [`Harness::session_usage`]).
//!
//! What Python keeps in four tables by harness (PERMISSION_ARGS, RESUME_ARGS, ADD_DIR_ARGS and SESSION_LOGS) is one
//! row per harness here, in [`HARNESSES`], so that adding a harness is adding a row.

use std::collections::HashSet;

use crate::constants::AGENT_KINDS;
use crate::error::OrchestratorError;
use serde_json::Value;
use crate::map::Map;

/// How each role's agent starts: the permission mode, and by role its model and its harness (kinds). A role missing
/// from models starts on its harness's default model, and one missing from kinds in DEFAULT_AGENT.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentSettings {
    /// --permission-mode as given, in Claude Code's names; None passes none to any harness.
    pub permission_mode: Option<String>,
    /// By role, the model its agent starts on.
    pub models: Map<String>,
    /// By role, the harness its agent starts in, one of AGENT_KINDS.
    pub kinds: Map<String>,
}

/// What the engine knows of one agent harness: how its agent starts (the arguments, and the environment its pane's
/// shell exports first), and its session log.
#[derive(Debug, PartialEq, Eq)]
pub struct Harness {
    /// Its name, as herdr's --kind and AGENT_KINDS have it.
    pub kind: &'static str,
    /// How it takes --permission-mode, which is in Claude Code's names.
    permissions: Permissions,
    /// How it resumes a saved session.
    pub(crate) resume: Resume,
    /// How it takes the model its agent starts on and a directory it is granted outside its working directory. A
    /// worktree run's handoff files are in the project's run directory, not the worktree.
    config: Config,
    /// Where and how it records the tokens of a session; None when it records none the engine reads, so that its
    /// turns' tokens are "unknown" (docs/design.md, Token usage).
    pub session_log: Option<SessionLog>,
}

/// How a harness takes --permission-mode.
#[derive(Debug, PartialEq, Eq)]
enum Permissions {
    /// As `--permission-mode <mode>`, every mode as it is named: Claude Code's own.
    AsNamed,
    /// By its own arguments for each mode it has an equivalent of; a mode not listed has none, and is not passed.
    Equivalents(&'static [(&'static str, &'static [&'static str])]),
}

/// How a harness resumes a saved session: by this word before the session id.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Resume {
    /// A flag, after the permission arguments.
    Flag(&'static str),
    /// A subcommand, which has to come before every other argument: codex's `resume`.
    Subcommand(&'static str),
}

/// How a harness takes the model its agent starts on and a directory it is granted.
#[derive(Debug, PartialEq, Eq)]
enum Config {
    /// By flags: `--model` before the model, and add_dir before the directory; add_dir None when the harness has no
    /// such flag.
    Flags { add_dir: Option<&'static str> },
    /// By opencode's JSON config in OPENCODE_CONFIG_CONTENT: opencode 2.0.24's TUI has no `--model` and no flag that
    /// grants a directory, and asks before writing outside its working directory. Only a `--standalone` opencode
    /// reads the variable; without that flag the TUI joins opencode's background service, whose config was read when
    /// the service started.
    OpencodeEnv,
}

/// How a role's agent starts in a harness.
#[derive(Debug, PartialEq, Eq)]
pub struct Launch {
    /// Its arguments, after herdr's `--`.
    pub args: Vec<String>,
    /// The environment variables, as (name, value), its pane's shell exports before it starts (Herdr::export).
    pub env: Vec<(String, String)>,
}

/// Where a harness keeps the record of a session's tokens, and how it writes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionLog {
    /// A glob relative to the home directory of the machine the agents run on, `{session}` standing for the session
    /// id herdr reports (crate::host::Host::session_log finds it).
    pub glob: &'static str,
    /// How it records the tokens of each answer of the model.
    pub format: LogFormat,
}

/// How a session log records the tokens of an answer of the model, one JSON object a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogFormat {
    /// Claude Code's: `{"type": "assistant", "timestamp", "message": {"id", "usage": {"input_tokens",
    /// "output_tokens", "cache_read_input_tokens", "cache_creation_input_tokens"}}}`, one line per content block of
    /// an answer, each with the whole answer's usage.
    ClaudeCode,
    /// pi's: `{"timestamp", "message": {"role": "assistant", "usage": {"input", "output", "cacheRead",
    /// "cacheWrite"}}}`, one line per answer.
    Pi,
}

/// Every harness a role can run in, in AGENT_KINDS' order.
pub const HARNESSES: [Harness; 5] = [
    Harness {
        kind: "claude",
        permissions: Permissions::AsNamed,
        resume: Resume::Flag("--resume"),
        config: Config::Flags { add_dir: Some("--add-dir") },
        session_log: Some(SessionLog { glob: ".claude/projects/*/{session}.jsonl", format: LogFormat::ClaudeCode }),
    },
    Harness {
        kind: "codex",
        // Codex asks before acting by default, so its default needs no flag.
        permissions: Permissions::Equivalents(&[
            ("default", &[]),
            ("acceptEdits", &["--full-auto"]),
            ("bypassPermissions", &["--dangerously-bypass-approvals-and-sandbox"]),
            ("plan", &["--sandbox", "read-only"]),
        ]),
        resume: Resume::Subcommand("resume"),
        config: Config::Flags { add_dir: Some("--add-dir") },
        session_log: None,
    },
    Harness {
        kind: "gemini",
        permissions: Permissions::Equivalents(&[
            ("default", &["--approval-mode", "default"]),
            ("acceptEdits", &["--approval-mode", "auto_edit"]),
            ("bypassPermissions", &["--approval-mode", "yolo"]),
        ]),
        resume: Resume::Flag("--resume"),
        config: Config::Flags { add_dir: Some("--include-directories") },
        session_log: None,
    },
    Harness {
        kind: "opencode",
        permissions: Permissions::Equivalents(&[]),
        resume: Resume::Flag("--session"),
        config: Config::OpencodeEnv,
        session_log: None,
    },
    Harness {
        kind: "pi",
        permissions: Permissions::Equivalents(&[]),
        resume: Resume::Flag("--session"),
        // pi asks before no tool call, inside its working directory or outside it, so it needs no grant.
        config: Config::Flags { add_dir: None },
        session_log: Some(SessionLog { glob: ".pi/agent/sessions/*/*_{session}.jsonl", format: LogFormat::Pi }),
    },
];

/// The harness of the name kind. Errors: kind is not one of AGENT_KINDS, as a hand-edited state.json can name; the
/// Python engine fails on it with a KeyError.
pub fn harness(kind: &str) -> Result<&'static Harness, OrchestratorError> {
    HARNESSES.iter().find(|h| h.kind == kind).ok_or_else(|| {
        OrchestratorError(format!(
            "agent {kind} is not a harness this orchestrator supports; it takes {}",
            AGENT_KINDS.join(", ")
        ))
    })
}

impl Harness {
    /// The harness's arguments for a --permission-mode, or None when it has no equivalent of mode.
    pub fn permission_args(&self, mode: &str) -> Option<Vec<String>> {
        match self.permissions {
            Permissions::AsNamed => Some(vec!["--permission-mode".to_string(), mode.to_string()]),
            Permissions::Equivalents(table) => {
                table.iter().find(|(m, _)| *m == mode).map(|(_, args)| args.iter().map(|a| a.to_string()).collect())
            }
        }
    }

    /// How a role's agent starts in this harness: with permission_mode's equivalent (none when the harness has none),
    /// into session if given one, on model if given one, and with access to extra_dir, and to every file under it,
    /// where the harness can grant it. An empty string is none, as Python takes it.
    pub fn launch(
        &self,
        permission_mode: Option<&str>,
        model: Option<&str>,
        session: Option<&str>,
        extra_dir: Option<&str>,
    ) -> Launch {
        fn given(value: Option<&str>) -> Option<&str> {
            value.filter(|v| !v.is_empty())
        }
        let (permission_mode, model) = (given(permission_mode), given(model));
        let (session, extra_dir) = (given(session), given(extra_dir));
        let mut args = permission_mode.and_then(|mode| self.permission_args(mode)).unwrap_or_default();
        if let Some(session) = session {
            match self.resume {
                Resume::Flag(flag) => args.extend([flag.to_string(), session.to_string()]),
                Resume::Subcommand(word) => {
                    args.splice(0..0, [word.to_string(), session.to_string()]);
                }
            }
        }
        let mut env = vec![];
        match self.config {
            Config::Flags { add_dir } => {
                if let Some(model) = model {
                    args.extend(["--model".to_string(), model.to_string()]);
                }
                if let (Some(dir), Some(flag)) = (extra_dir, add_dir) {
                    args.extend([flag.to_string(), dir.to_string()]);
                }
            }
            Config::OpencodeEnv => {
                let mut config = serde_json::Map::new();
                if let Some(model) = model {
                    config.insert("model".to_string(), model.into());
                }
                if let Some(dir) = extra_dir {
                    let grant = serde_json::json!({ "external_directory": { format!("{dir}/*"): "allow" } });
                    config.insert("permission".to_string(), grant);
                }
                if !config.is_empty() {
                    args.push("--standalone".to_string());
                    // A backslash as \u005c, not \\, which fish's single quotes would read as one (Herdr::export).
                    let json = Value::Object(config).to_string().replace(r"\\", r"\u005c");
                    env.push(("OPENCODE_CONFIG_CONTENT".to_string(), json));
                }
            }
        }
        Launch { args, env }
    }

    /// The token usage this harness's session log, as its SessionLog finds it, records for each answer of the model,
    /// oldest first; none for a harness without a session log. log is the file as it is on disk, each line read as
    /// read as JSON with each lone surrogate as U+FFFD (without_lone_surrogates), as each byte that is not UTF-8
    /// is too. A line that is not a record of an answer, or whose counts are not all integers, is skipped. Claude
    /// Code writes one line per content block of an answer, each with the answer's usage, so each answer counts once,
    /// at its first line that names its usage and time.
    pub fn session_usage(&self, log: &[u8]) -> Vec<Usage> {
        let Some(SessionLog { format, .. }) = self.session_log else { return vec![] };
        // The ids of the Claude Code answers counted so far.
        let mut seen = HashSet::new();
        let mut out = vec![];
        for line in log.split(|b| *b == b'\n') {
            let Some(usage) = answer_usage(format, &String::from_utf8_lossy(line), &mut seen) else { continue };
            out.push(usage);
        }
        out
    }
}

/// The tokens of one answer of the model, as a session log records them.
#[derive(Clone, Debug, PartialEq)]
pub struct Usage {
    /// When it was written, in seconds since the epoch, as Python's datetime.timestamp() gives them.
    pub when: f64,
    /// Its tokens, by kind.
    pub tokens: Tokens,
}

/// Token counts, by kind, as metrics.json writes them. A count is whatever integer the log holds; sums saturate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tokens {
    /// Input tokens not read from the cache: Claude Code's input_tokens, pi's input.
    pub input: i64,
    /// Output tokens: Claude Code's output_tokens, pi's output.
    pub output: i64,
    /// Input tokens read from the cache: Claude Code's cache_read_input_tokens, pi's cacheRead; 0 when not logged.
    pub cache_read: i64,
    /// Input tokens written to the cache: Claude Code's cache_creation_input_tokens, pi's cacheWrite; 0 when not
    /// logged.
    pub cache_write: i64,
}

impl Tokens {
    /// The counts as metrics.json and state.json write them: `{"input", "output", "cache_read", "cache_write"}`.
    pub fn to_json(self) -> Value {
        serde_json::json!({
            "input": self.input,
            "output": self.output,
            "cache_read": self.cache_read,
            "cache_write": self.cache_write,
        })
    }

    /// These counts and other's, added.
    pub fn plus(self, other: Tokens) -> Tokens {
        Tokens {
            input: self.input.saturating_add(other.input),
            output: self.output.saturating_add(other.output),
            cache_read: self.cache_read.saturating_add(other.cache_read),
            cache_write: self.cache_write.saturating_add(other.cache_write),
        }
    }
}

/// A Claude Code message id as Python's set of them tells ids apart: a number equal to another is the same id, true
/// being 1 and false 0, and a missing id the same as null.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum MessageId {
    None,
    Str(String),
    /// A whole number, whether it was written as an int, a float or a bool.
    Int(i128),
    /// The bits of any other float; one JSON reads is never NaN, and -0.0 is Int(0).
    Float(u64),
}

impl MessageId {
    /// The id value is; None for a list or an object, which a set cannot hold (Python's TypeError).
    fn of(value: Option<&Value>) -> Option<MessageId> {
        Some(match value {
            None | Some(Value::Null) => MessageId::None,
            Some(Value::String(s)) => MessageId::Str(s.clone()),
            Some(Value::Bool(b)) => MessageId::Int(i128::from(*b)),
            Some(Value::Number(n)) => match (n.as_i64(), n.as_u64(), n.as_f64()) {
                (Some(i), _, _) => MessageId::Int(i.into()),
                (None, Some(u), _) => MessageId::Int(u.into()),
                (None, None, Some(x)) if x.fract() == 0.0 && x.abs() < 1e38 => MessageId::Int(x as i128),
                (None, None, x) => MessageId::Float(x.unwrap_or(f64::NAN).to_bits()),
            },
            Some(Value::Array(_) | Value::Object(_)) => return None,
        })
    }
}

/// The usage one line of a session log in format records, as session_usage reads it: None for a line that is not a
/// record of an answer, as Python's loop skips it. seen holds the ids of the Claude Code answers counted so far, and
/// gains this line's, as Python's set does, before its counts are read.
fn answer_usage(format: LogFormat, line: &str, seen: &mut HashSet<MessageId>) -> Option<Usage> {
    // Each step in the order Python's takes it: a step that fails skips the line, and the steps before it have had
    // their effect on seen.
    let entry: Value = serde_json::from_str(&without_lone_surrogates(line)).ok()?;
    let entry = entry.as_object()?;
    let message = entry.get("message")?.as_object()?;
    let usage = message.get("usage")?;
    let when = crate::summary::epoch_time(entry.get("timestamp")?.as_str()?).ok()?;
    let keys = match format {
        LogFormat::ClaudeCode => {
            if entry.get("type").and_then(Value::as_str) != Some("assistant") {
                return None;
            }
            if !seen.insert(MessageId::of(message.get("id"))?) {
                return None;
            }
            ["input_tokens", "output_tokens", "cache_read_input_tokens", "cache_creation_input_tokens"]
        }
        LogFormat::Pi => {
            if message.get("role").and_then(Value::as_str) != Some("assistant") {
                return None;
            }
            ["input", "output", "cacheRead", "cacheWrite"]
        }
    };
    // The input and output counts must be there, and the cache counts are 0 when they are not; each is an int, not a
    // bool or a float (Python's `type(v) is int`).
    let usage = usage.as_object()?;
    let (input, output) = (usage.get(keys[0])?, usage.get(keys[1])?);
    let cache = |key: &str| usage.get(key).map_or(Some(0), Value::as_i64);
    let tokens = Tokens {
        input: input.as_i64()?,
        output: output.as_i64()?,
        cache_read: cache(keys[2])?,
        cache_write: cache(keys[3])?,
    };
    Some(Usage { when, tokens })
}

/// JSON text with each lone surrogate escape in its strings (\\ud800 to \\udfff, not half of a high-low pair) made
/// \\ufffd: a session log that JSON.stringify wrote from a string cut in the middle of a pair holds them, and
/// serde_json refuses them. Text that is not JSON is left so, for the parser to refuse.
fn without_lone_surrogates(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains("\\u") {
        return text.into();
    }
    // The code unit of the \\uXXXX escape at byte i of text, if one is there.
    let escape = |i: usize| -> Option<u32> {
        let hex = text.get(i..i + 6)?.strip_prefix("\\u")?;
        hex.bytes().all(|b| b.is_ascii_hexdigit()).then(|| u32::from_str_radix(hex, 16).ok())?
    };
    let bytes = text.as_bytes();
    let (mut out, mut copied, mut i, mut in_string) = (String::new(), 0, 0, false);
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                in_string = !in_string;
                i += 1;
            }
            b'\\' if in_string => match escape(i) {
                Some(0xD800..=0xDBFF) if matches!(escape(i + 6), Some(0xDC00..=0xDFFF)) => i += 12,
                Some(0xD800..=0xDFFF) => {
                    out.push_str(&text[copied..i]);
                    out.push_str("\\ufffd");
                    i += 6;
                    copied = i;
                }
                _ => i += 2,
            },
            _ => i += 1,
        }
    }
    if copied == 0 {
        return text.into();
    }
    out.push_str(&text[copied..]);
    out.into()
}

#[cfg(test)]
mod tests;
