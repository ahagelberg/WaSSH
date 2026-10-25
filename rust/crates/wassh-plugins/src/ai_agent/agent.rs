//! The conversation and the tool loop.
//!
//! Port of the parts of `main.ts` that drive a turn: build the request, stream
//! the answer, then run whatever tools the model asked for — asking first when
//! the rules say so — and feed the results back until it answers in prose.

use std::collections::VecDeque;
use std::sync::mpsc::Receiver;

use serde_json::Value;

use super::protocol::{
    ApiMessage, ApiToolCall, Completion, TOOL_ASK_USER, TOOL_CURRENT_TIME, TOOL_DEV_CREATE,
    TOOL_DEV_DIFF, TOOL_DEV_EDIT, TOOL_DEV_FIND, TOOL_DEV_GREP, TOOL_DEV_LIST, TOOL_DEV_VIEW,
    TOOL_LOCAL_EDIT, TOOL_LOCAL_LIST, TOOL_LOCAL_READ, TOOL_LOCAL_WRITE, TOOL_RAG_SEARCH,
    TOOL_REMOTE_DELETE, TOOL_REMOTE_EDIT, TOOL_REMOTE_LIST, TOOL_REMOTE_READ, TOOL_REMOTE_WRITE,
    TOOL_RUN_COMMAND, TOOL_SYSTEM_DISK, TOOL_SYSTEM_MEMORY, TOOL_SYSTEM_OVERVIEW,
    TOOL_SYSTEM_PROCESSES, TOOL_TERMINAL_KEYS, TOOL_TERMINAL_READ, TOOL_TERMINAL_WAIT,
    TOOL_TERMINAL_WRITE, TOOL_WEB_FETCH, TOOL_WEB_SEARCH,
};
use super::providers::{complete, embeddings, CompletionRequest, EmbedConfig, StreamEvent};
use super::rag;
use super::rules::{
    approval_scopes, decide_command, ApprovalScope, RuleDecision, ALLOW_ALL_PATTERN,
};
use super::tools::{self, ToolResult};
use super::web;
use crate::plugin::SessionAccess;

/// The session slot every dispatch takes.
///
/// Two lifetimes on purpose: the borrow of the slot must be allowed to end long
/// before the session itself does, or nothing else could touch the context.
type Session<'a, 'b> = &'a mut Option<&'b mut dyn SessionAccess>;

/// Longest command output handed to the model.
pub const MAX_OUTPUT_CHARS: usize = 64_000;
/// How much of the raw tail is kept so the markers are never cut off.
pub const MARKER_TAIL_CHARS: usize = 4_000;
/// Key the session caches the running command under.
pub const EXEC_KEY: &str = "ai.run_command";
/// Key the session caches a probe under (existence checks and system reports).
pub const PROBE_KEY: &str = "ai.probe";
/// Marker the sudo preamble prints when the password was refused.
pub const SUDO_AUTH_FAILED_MARKER: &str = "__WASSH_SUDO_FAILED__";
/// Longest a `terminal_wait` blocks by default.
pub const TERMINAL_WAIT_DEFAULT_MS: u64 = 5_000;
/// System prompt used when summarising a chat for a new one.
pub const SUMMARY_SYSTEM_PROMPT: &str = "Summarise the conversation above for a fresh agent session that must \
continue the work. Reply with terse bullet points, at most 150 words, covering: Goal (what the user wants), \
State (what has been done and what was learned, including hosts, paths and commands that matter), Next (the \
immediate next step). No preamble, no closing sentence.";

/// One line of the transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Turn {
    /// What the user asked.
    User(String),
    /// What the model answered.
    Assistant {
        /// The text so far.
        text: String,
        /// Whether the user stopped it early.
        stopped: bool,
    },
    /// A tool the agent ran.
    Tool {
        /// Tool name.
        name: String,
        /// One-line summary of the call, e.g. `$ uptime`.
        label: String,
        /// What it returned.
        output: String,
        /// Whether it failed.
        failed: bool,
    },
    /// A call the rules refused.
    Denied(String),
    /// A question the agent asked, and the answer once given.
    Question {
        /// What it asked.
        text: String,
        /// The user's answer, when it came.
        answer: Option<String>,
    },
    /// Something that went wrong.
    Error(String),
    /// A note from a slash command.
    Notice(String),
}

/// What the agent is doing right now.
pub enum Phase {
    /// Waiting for the user.
    Idle,
    /// A reply is streaming in.
    Streaming {
        /// Where the text goes.
        turn: usize,
        /// Events from the worker thread.
        events: Receiver<StreamEvent>,
    },
    /// A command needs the user's permission.
    Approving {
        /// The call being approved.
        call: ApiToolCall,
        /// The command it wants to run.
        command: String,
        /// "Always allow/deny" choices offered.
        scopes: Vec<ApprovalScope>,
    },
    /// A tool is running.
    Running {
        /// The call being answered.
        call: ApiToolCall,
        /// Where its transcript line is.
        turn: usize,
        /// What the agent is waiting for.
        waiting: Waiting,
    },
    /// The agent asked the user a question.
    Asking {
        /// The call being answered.
        call: ApiToolCall,
        /// The question.
        text: String,
    },
}

/// What a pending tool waits on.
pub enum Waiting {
    /// A shell command on the session.
    Command {
        /// Cache key it answers under.
        key: &'static str,
        /// What to do with the output.
        then: CommandThen,
    },
    /// A remote read.
    Read {
        /// File being read.
        path: String,
        /// Cap on the bytes.
        max: u64,
        /// What to do with the contents.
        then: ReadThen,
    },
    /// A remote listing.
    List {
        /// Directory being listed.
        path: String,
    },
    /// An upload in flight.
    Transfer {
        /// Where the bytes go.
        target: String,
        /// Text to report instead of the transfer line (the diff of an edit).
        note: Option<String>,
    },
    /// A worker thread (web, knowledge base).
    Worker {
        /// Its answer.
        events: Receiver<ToolResult>,
    },
    /// The terminal screen, until it changes.
    Terminal {
        /// The screen before the wait.
        before: String,
        /// When the wait gives up, in milliseconds since the epoch.
        until_ms: u64,
    },
}

/// What follows a probe command.
#[derive(Clone, Debug)]
pub enum CommandThen {
    /// Return the output.
    Return,
    /// Write a remote file once the probe says whether it exists.
    WriteRemote {
        /// Destination.
        path: String,
        /// Contents.
        content: String,
        /// Whether an existing file may be replaced.
        overwrite: bool,
    },
}

/// What follows a remote read.
#[derive(Clone, Debug)]
pub enum ReadThen {
    /// Return the contents.
    Return,
    /// Replace text and write the file back.
    Edit {
        /// Text to find.
        old: String,
        /// Its replacement.
        new: String,
    },
}

/// Settings the tools read.
#[derive(Clone, Debug, Default)]
pub struct ToolSettings {
    /// Search engine for `web_search`.
    pub web_provider: Option<String>,
    /// Its key.
    pub web_key: Option<String>,
    /// Knowledge-base folders.
    pub rag_folders: Vec<String>,
    /// Embedding model, when semantic search is configured.
    pub embed: Option<EmbedConfig>,
}

/// One conversation's state.
#[derive(Default)]
pub struct Agent {
    /// The transcript.
    pub turns: Vec<Turn>,
    /// The conversation in the provider's shape.
    pub history: Vec<ApiMessage>,
    /// What the agent is doing.
    pub phase: Option<Phase>,
    /// Last failure, shown under the transcript.
    pub error: Option<String>,
    /// Directory commands run in.
    pub cwd: String,
    /// Rules shipped with the app, from the plugin settings.
    pub app_allow: Vec<String>,
    /// Patterns the app refuses, from the plugin settings.
    pub app_deny: Vec<String>,
    /// Rules for this host.
    pub host_allow: Vec<String>,
    /// Patterns this host refuses.
    pub host_deny: Vec<String>,
    /// Rules the user approved for this session.
    pub session_allow: Vec<String>,
    /// Rules the user refused for this session.
    pub session_deny: Vec<String>,
    /// Tool settings from the plugin data.
    pub tools: ToolSettings,
    /// Set when the model owes an answer to a tool result.
    pub needs_reply: bool,
    /// Clock used by the time-based tools; the view sets it every frame.
    pub now_ms: u64,
    /// The last command, kept so a refused `sudo` can be retried.
    pub last_command: Option<String>,
    /// Set when the last command failed because `sudo` wanted a password.
    pub sudo_waiting: bool,
    /// Tool calls still to run for the current reply.
    queue: VecDeque<ApiToolCall>,
}

impl Agent {
    /// A fresh conversation.
    pub fn new() -> Self {
        Self {
            cwd: "/".to_owned(),
            ..Self::default()
        }
    }

    /// Whether a reply or tool is in flight.
    pub fn busy(&self) -> bool {
        matches!(
            self.phase,
            Some(Phase::Streaming { .. } | Phase::Running { .. })
        )
    }

    /// Whether the user has to answer an approval.
    pub fn approving(&self) -> bool {
        matches!(self.phase, Some(Phase::Approving { .. }))
    }

    /// Whether the agent is waiting on the user.
    pub fn asking(&self) -> bool {
        matches!(self.phase, Some(Phase::Asking { .. }))
    }

    /// Whether anything at all blocks a new message.
    pub fn blocked(&self) -> bool {
        self.busy() || self.approving() || self.asking()
    }

    /// Throw the conversation away.
    pub fn clear(&mut self) {
        self.turns.clear();
        self.history.clear();
        self.phase = None;
        self.error = None;
        self.queue.clear();
        self.needs_reply = false;
    }

    /// Add the user's turn, if the conversation is idle.
    pub fn push_user(&mut self, text: &str) -> bool {
        let text = text.trim();
        if text.is_empty() || self.blocked() {
            return false;
        }
        self.error = None;
        self.turns.push(Turn::User(text.to_owned()));
        self.history.push(ApiMessage::User {
            content: text.to_owned(),
        });
        true
    }

    /// Start a reply for the conversation as it stands.
    pub fn start(&mut self, request: CompletionRequest) {
        self.turns.push(Turn::Assistant {
            text: String::new(),
            stopped: false,
        });
        let turn = self.turns.len() - 1;
        let (sender, events) = std::sync::mpsc::channel();
        std::thread::spawn(move || complete(&request, &sender));
        self.phase = Some(Phase::Streaming { turn, events });
    }

    /// Take in whatever the worker thread or the session has produced.
    pub fn poll(&mut self, session: Session<'_, '_>) -> bool {
        let mut progressed = false;
        match &self.phase {
            Some(Phase::Streaming { turn, .. }) => {
                let turn = *turn;
                let mut done: Option<Result<Completion, String>> = None;
                if let Some(Phase::Streaming { events, .. }) = &self.phase {
                    loop {
                        match events.try_recv() {
                            Ok(StreamEvent::Delta(text)) => {
                                if let Some(Turn::Assistant { text: body, .. }) =
                                    self.turns.get_mut(turn)
                                {
                                    body.push_str(&text);
                                }
                                progressed = true;
                            }
                            Ok(StreamEvent::Done(completion)) => {
                                done = Some(Ok(completion));
                                break;
                            }
                            Ok(StreamEvent::Failed(error)) => {
                                done = Some(Err(error));
                                break;
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => break,
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                done = Some(Err("the provider closed the connection".to_owned()));
                                break;
                            }
                        }
                    }
                }
                if let Some(result) = done {
                    self.phase = None;
                    self.finish_reply(turn, result, session);
                    progressed = true;
                }
            }
            Some(Phase::Running { .. }) => progressed = self.poll_tool(session),
            _ => {}
        }
        progressed
    }

    /// Poll the running tool and move the turn on when it answers.
    fn poll_tool(&mut self, session: Session<'_, '_>) -> bool {
        enum Ready {
            Settle(String, String, bool),
            WriteRemote(String, String),
        }

        let Some(Phase::Running {
            call,
            turn,
            waiting,
        }) = &self.phase
        else {
            return false;
        };
        let (call, turn) = (call.clone(), *turn);
        let ready = match waiting {
            Waiting::Command { key, then } => {
                let (key, then) = (*key, then.clone());
                let (output, failure) = match session.as_ref() {
                    Some(access) => (
                        access.output(key).cloned(),
                        access.failure(key).map(str::to_owned),
                    ),
                    None => (None, Some("The session is not connected.".to_owned())),
                };
                if output.is_none() && failure.is_none() {
                    return false;
                }
                match then {
                    CommandThen::Return => {
                        let (visible, exit_code, directory, truncated) = match (output, failure) {
                            (Some(output), _) => {
                                let raw = tail_of(&format!("{}{}", output.stdout, output.stderr));
                                let (visible, exit_code, directory) = unwrap_output(&raw);
                                (
                                    clamp_output(visible),
                                    exit_code,
                                    directory,
                                    raw.chars().count() > MAX_OUTPUT_CHARS,
                                )
                            }
                            (None, Some(failure)) => (failure, None, None, false),
                            (None, None) => (String::new(), None, None, false),
                        };
                        if let Some(directory) = directory
                            && !directory.is_empty()
                        {
                            self.cwd = directory;
                        }
                        // A refused sudo password is the user's to resolve.
                        let refused_sudo = visible.contains(SUDO_AUTH_FAILED_MARKER);
                        if refused_sudo {
                            self.sudo_waiting = true;
                            self.error = Some("sudo needs your password.".to_owned());
                            self.needs_reply = false;
                        }
                        let visible = visible.replace(SUDO_AUTH_FAILED_MARKER, "").trim_end().to_owned();
                        Ready::Settle(
                            self.label(turn),
                            command_result_text(&visible, exit_code, truncated),
                            exit_code.is_some_and(|code| code != 0) && !refused_sudo,
                        )
                    }
                    CommandThen::WriteRemote {
                        path,
                        content,
                        overwrite,
                    } => {
                        let exists = output
                            .as_ref()
                            .is_some_and(|output| output.stdout.trim_end().ends_with("yes"));
                        if exists && !overwrite {
                            Ready::Settle(
                                format!("write {path}"),
                                format!("{path} already exists; pass overwrite to replace it."),
                                true,
                            )
                        } else {
                            Ready::WriteRemote(path, content)
                        }
                    }
                }
            }
            Waiting::Read { path, max, then } => {
                let (path, max, then) = (path.clone(), *max, then.clone());
                let content = session.as_mut().and_then(|access| access.read(&path, max));
                let Some(content) = content else {
                    return false;
                };
                let text = String::from_utf8_lossy(&content.data).into_owned();
                match then {
                    ReadThen::Return => {
                        let mut body = text;
                        if content.truncated {
                            body.push_str("\n\n[Content truncated at the read limit]");
                        }
                        Ready::Settle(format!("read {path}"), body, false)
                    }
                    ReadThen::Edit { old, new } => {
                        if content.truncated {
                            Ready::Settle(
                                format!("edit {path}"),
                                format!("{path} is larger than the edit limit."),
                                true,
                            )
                        } else {
                            match replace_and_report(&text, &old, &new) {
                                Ok((updated, diff)) => Ready::WriteRemote(path, format!("{updated}\u{0}{diff}")),
                                Err(error) => Ready::Settle(format!("edit {path}"), error, true),
                            }
                        }
                    }
                }
            }
            Waiting::List { path } => {
                let path = path.clone();
                let listed = session
                    .as_ref()
                    .and_then(|access| access.listing())
                    .filter(|listing| listing.path == path)
                    .cloned();
                let Some(listing) = listed else {
                    return false;
                };
                let mut lines = Vec::new();
                for entry in listing.entries.iter().take(tools::MAX_DIR_ENTRIES) {
                    let kind = if entry.is_dir { "dir " } else { "file" };
                    let size = if entry.is_dir {
                        String::new()
                    } else {
                        format!("{:>10}", entry.size)
                    };
                    lines.push(format!("{kind}{size}  {}", entry.name));
                }
                if lines.is_empty() {
                    lines.push("(empty directory)".to_owned());
                }
                Ready::Settle(format!("list {}", listing.path), lines.join("\n"), false)
            }
            Waiting::Transfer { target, note } => {
                let (target, note) = (target.clone(), note.clone());
                let busy = session.as_ref().is_some_and(|access| access.is_transferring());
                if busy {
                    return false;
                }
                let status = session
                    .as_ref()
                    .and_then(|access| access.transfer_status().map(str::to_owned))
                    .unwrap_or_default();
                let lower = status.to_lowercase();
                let failed = lower.contains("fail") || lower.contains("denied") || lower.contains("error");
                match note {
                    Some(note) => {
                        let body = if failed {
                            format!("{note}\n\n[upload failed: {status}]")
                        } else {
                            note
                        };
                        Ready::Settle(format!("edit {target}"), body, failed)
                    }
                    None => Ready::Settle(
                        format!("write {target}"),
                        if status.is_empty() {
                            format!("Wrote {target}.")
                        } else if failed {
                            status
                        } else {
                            format!("Wrote {target}. {status}")
                        },
                        failed,
                    ),
                }
            }
            Waiting::Worker { events } => match events.try_recv() {
                Ok(Ok(output)) => Ready::Settle(self.label(turn), output, false),
                Ok(Err(error)) => Ready::Settle(self.label(turn), error, true),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Ready::Settle(
                    self.label(turn),
                    "The tool stopped without an answer.".to_owned(),
                    true,
                ),
                Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            },
            Waiting::Terminal { before, until_ms } => {
                let (before, until_ms) = (before.clone(), *until_ms);
                let screen = session
                    .as_ref()
                    .map(|access| access.screen_text())
                    .unwrap_or_default();
                let changed = screen != before;
                if !changed && self.now_ms < until_ms {
                    return false;
                }
                let body = if changed {
                    format!("The terminal changed. Current screen:\n{screen}")
                } else {
                    format!("No change within the timeout. Current screen:\n{screen}")
                };
                Ready::Settle("terminal wait".to_owned(), body, false)
            }
        };

        match ready {
            Ready::Settle(label, output, failed) => {
                self.phase = None;
                self.settle(call, turn, label, output, failed, session);
                // A refused sudo password waits for the user, not the model.
                if self.sudo_waiting {
                    self.needs_reply = false;
                }
            }
            Ready::WriteRemote(target, payload) => {
                // An edit packs the diff after a NUL so the file keeps its bytes.
                let (content, diff) = match payload.split_once('\u{0}') {
                    Some((content, diff)) => (content.to_owned(), Some(diff.to_owned())),
                    None => (payload, None),
                };
                let Some(access) = session.as_mut() else {
                    self.phase = None;
                    self.settle(
                        call,
                        turn,
                        format!("write {target}"),
                        "The session is not connected.".to_owned(),
                        true,
                        session,
                    );
                    return true;
                };
                let local = std::env::temp_dir().join(format!(
                    "wassh-agent-{}-{}.tmp",
                    std::process::id(),
                    self.now_ms
                ));
                if let Err(error) = std::fs::write(&local, &content) {
                    self.phase = None;
                    self.settle(
                        call,
                        turn,
                        format!("write {target}"),
                        format!("Cannot stage the file: {error}"),
                        true,
                        session,
                    );
                    return true;
                }
                access.upload(&local.display().to_string(), &target);
                let _ = std::fs::remove_file(&local);
                // The upload finishes on a later frame; a diff is the answer.
                if let Some(Turn::Tool { output, .. }) = self.turns.get_mut(turn) {
                    *output = diff.clone().unwrap_or_else(|| format!("Uploading to {target}…"));
                }
                self.phase = Some(Phase::Running {
                    call,
                    turn,
                    waiting: Waiting::Transfer { target, note: diff },
                });
            }
        }
        true
    }

    /// The label of a tool turn, for the transcript.
    fn label(&self, turn: usize) -> String {
        self.turns
            .get(turn)
            .and_then(|turn| match turn {
                Turn::Tool { label, .. } => Some(label.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// Record a tool result and move on to the next call or the next reply.
    fn settle(
        &mut self,
        call: ApiToolCall,
        turn: usize,
        label: String,
        output: String,
        failed: bool,
        session: Session<'_, '_>,
    ) {
        if let Some(Turn::Tool {
            label: turn_label,
            output: turn_output,
            failed: turn_failed,
            ..
        }) = self.turns.get_mut(turn)
        {
            *turn_label = label;
            *turn_output = output.clone();
            *turn_failed = failed;
        } else {
            self.turns.push(Turn::Tool {
                name: call.name.clone(),
                label,
                output: output.clone(),
                failed,
            });
        }
        self.history.push(ApiMessage::Tool {
            tool_call_id: call.id.clone(),
            content: output,
        });
        self.advance(session);
    }

    /// Run the next queued call, or ask the model again.
    fn advance(&mut self, session: Session<'_, '_>) {
        if let Some(next) = self.queue.pop_front() {
            self.begin_tool(next, session);
        } else {
            self.needs_reply = true;
        }
    }

    /// Handle a finished reply: either the turn is over or tools must run.
    fn finish_reply(
        &mut self,
        turn: usize,
        result: Result<Completion, String>,
        session: Session<'_, '_>,
    ) {
        match result {
            Err(error) => {
                self.error = Some(error.clone());
                self.turns.push(Turn::Error(error));
            }
            Ok(completion) => {
                if let Some(Turn::Assistant { text, .. }) = self.turns.get_mut(turn)
                    && text.is_empty()
                {
                    text.clone_from(&completion.text);
                }
                let text = self
                    .turns
                    .get(turn)
                    .and_then(|turn| match turn {
                        Turn::Assistant { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                self.history.push(ApiMessage::Assistant {
                    content: text,
                    tool_calls: completion.tool_calls.clone(),
                });
                self.queue = completion.tool_calls.into_iter().collect();
                if let Some(next) = self.queue.pop_front() {
                    self.begin_tool(next, session);
                }
            }
        }
    }

    /// Start one tool call, from any entry point.
    fn begin_tool(&mut self, call: ApiToolCall, session: Session<'_, '_>) {
        self.turns.push(Turn::Tool {
            name: call.name.clone(),
            label: call_label(&call),
            output: String::new(),
            failed: false,
        });
        let turn = self.turns.len() - 1;
        self.dispatch(call, turn, session);
    }

    /// Run one call, or park the agent on whatever it needs.
    fn dispatch(&mut self, call: ApiToolCall, turn: usize, session: Session<'_, '_>) {
        let arguments: Value = serde_json::from_str(&call.arguments).unwrap_or(Value::Null);
        let name = call.name.clone();
        if !tools::is_tool(&name) {
            self.settle(
                call,
                turn,
                name.clone(),
                format!("The tool {name} is not available."),
                true,
                session,
            );
            return;
        }

        // Everything that needs the session checks once, up front.
        // `run_command` asks for approval first, so it keeps its own check.
        let needs_session_now = tools::needs_session(&name)
            && !matches!(name.as_str(), TOOL_ASK_USER | TOOL_RUN_COMMAND);
        if needs_session_now && session.is_none() {
            let label = call_label(&call);
            self.settle(
                call,
                turn,
                label,
                "The session is not connected.".to_owned(),
                true,
                session,
            );
            return;
        }

        match name.as_str() {
            TOOL_ASK_USER => {
                let text = arguments["text"]
                    .as_str()
                    .filter(|text| !text.trim().is_empty())
                    .unwrap_or("The agent needs your input.")
                    .to_owned();
                self.turns.push(Turn::Question {
                    text: text.clone(),
                    answer: None,
                });
                self.phase = Some(Phase::Asking { call, text });
            }
            TOOL_RUN_COMMAND => {
                let Some(command) = arguments["command"]
                    .as_str()
                    .map(str::trim)
                    .filter(|command| !command.is_empty())
                    .map(str::to_owned)
                else {
                    self.settle(
                        call,
                        turn,
                        "run_command".to_owned(),
                        "The command argument was missing or empty.".to_owned(),
                        true,
                        session,
                    );
                    return;
                };
                let decision = decide_command(
                    &command,
                    Some(&self.host_allow),
                    Some(&self.host_deny),
                    Some(&self.app_allow),
                    Some(&self.app_deny),
                    &self.session_allow,
                    &self.session_deny,
                );
                let sudo_stdin = arguments["sudoStdin"]
                    .as_str()
                    .filter(|password| !password.is_empty())
                    .map(str::to_owned);
                match decision {
                    RuleDecision::Allow => {
                        self.exec_command(call, command, sudo_stdin, turn, session)
                    }
                    RuleDecision::Deny => {
                        let reason = format!("Running `{command}` is not allowed by the permission rules.");
                        self.settle(call, turn, format!("$ {command}"), reason, true, session);
                    }
                    RuleDecision::Ask => {
                        self.phase = Some(Phase::Approving {
                            scopes: approval_scopes(&command),
                            call,
                            command,
                        });
                    }
                }
            }
            _ => {
                if let Some(command) = tools::remote_shell_command(&name, &arguments) {
                    self.run_probe(call, turn, CommandThen::Return, command, session);
                    return;
                }
                self.dispatch_remote(call, turn, &name, &arguments, session);
            }
        }
    }

    /// The tools that need the session or a worker thread.
    fn dispatch_remote(
        &mut self,
        call: ApiToolCall,
        turn: usize,
        name: &str,
        arguments: &Value,
        session: Session<'_, '_>,
    ) {
        match name {
            TOOL_REMOTE_READ => {
                let path = relative_remote(&self.cwd, arguments["path"].as_str().unwrap_or_default());
                let max = arguments["maxChars"]
                    .as_u64()
                    .unwrap_or(32_000)
                    .clamp(1_000, tools::MAX_FILE_READ_CHARS as u64);
                self.start_read(call, turn, path, max, ReadThen::Return, session);
            }
            TOOL_REMOTE_EDIT => {
                let path = relative_remote(&self.cwd, arguments["path"].as_str().unwrap_or_default());
                let old = arguments["oldText"].as_str().unwrap_or_default().to_owned();
                let new = arguments["newText"].as_str().unwrap_or_default().to_owned();
                if old.is_empty() {
                    self.settle(
                        call,
                        turn,
                        format!("edit {path}"),
                        "oldText must not be empty.".to_owned(),
                        true,
                        session,
                    );
                    return;
                }
                self.start_read(
                    call,
                    turn,
                    path,
                    tools::MAX_FILE_READ_CHARS as u64,
                    ReadThen::Edit { old, new },
                    session,
                );
            }
            TOOL_REMOTE_WRITE => {
                let path = relative_remote(&self.cwd, arguments["path"].as_str().unwrap_or_default());
                let content = arguments["content"].as_str().unwrap_or_default().to_owned();
                let overwrite = arguments["overwrite"].as_bool().unwrap_or(false);
                let probe = format!("test -e {} && echo yes || echo no", shell_quote(&path));
                self.run_probe(
                    call,
                    turn,
                    CommandThen::WriteRemote {
                        path,
                        content,
                        overwrite,
                    },
                    probe,
                    session,
                );
            }
            TOOL_REMOTE_LIST => {
                let path = relative_remote(&self.cwd, arguments["path"].as_str().unwrap_or_default());
                match session.as_mut() {
                    Some(access) => access.request_listing(&path),
                    None => {
                        self.settle(
                            call,
                            turn,
                            format!("list {path}"),
                            "The session is not connected.".to_owned(),
                            true,
                            session,
                        );
                        return;
                    }
                }
                self.phase = Some(Phase::Running {
                    call,
                    turn,
                    waiting: Waiting::List { path },
                });
            }
            TOOL_REMOTE_DELETE => {
                let path = relative_remote(&self.cwd, arguments["path"].as_str().unwrap_or_default());
                let command = format!("rm -rf -- {} && echo removed", shell_quote(&path));
                self.run_probe(call, turn, CommandThen::Return, command, session);
            }
            TOOL_TERMINAL_READ => {
                let screen = session
                    .as_ref()
                    .map(|access| access.screen_text())
                    .unwrap_or_default();
                let lines: Vec<&str> = screen.lines().collect();
                let keep = arguments["maxLines"]
                    .as_u64()
                    .map_or(lines.len(), |value| value as usize)
                    .clamp(1, 5_000);
                let from = lines.len().saturating_sub(keep);
                let body = if lines.is_empty() {
                    "The terminal is empty.".to_owned()
                } else {
                    lines[from..].join("\n")
                };
                self.settle(call, turn, "terminal_read".to_owned(), body, false, session);
            }
            TOOL_TERMINAL_WRITE | TOOL_TERMINAL_KEYS => {
                let text = if name == TOOL_TERMINAL_KEYS {
                    match tools::keys_sequence(arguments["keys"].as_str().unwrap_or_default()) {
                        Ok(sequence) => sequence,
                        Err(error) => {
                            let label = call_label(&call);
                            self.settle(call, turn, label, error, true, session);
                            return;
                        }
                    }
                } else {
                    arguments["text"].as_str().unwrap_or_default().to_owned()
                };
                let enter = arguments["pressEnter"].as_bool().unwrap_or(false);
                let label = call_label(&call);
                let Some(access) = session.as_mut() else {
                    self.settle(
                        call,
                        turn,
                        label,
                        "The session is not connected.".to_owned(),
                        true,
                        session,
                    );
                    return;
                };
                access.type_text(&text);
                if enter {
                    access.type_text("\r");
                }
                let note = if name == TOOL_TERMINAL_KEYS {
                    format!("Sent {}.", arguments["keys"].as_str().unwrap_or_default())
                } else {
                    format!(
                        "Typed {} characters{}.",
                        text.chars().count(),
                        if enter { " and Enter" } else { "" }
                    )
                };
                self.settle(call, turn, label, note, false, session);
            }
            TOOL_TERMINAL_WAIT => {
                let timeout = arguments["timeoutMs"]
                    .as_u64()
                    .unwrap_or(TERMINAL_WAIT_DEFAULT_MS)
                    .clamp(100, tools::MAX_TERMINAL_WAIT_MS);
                let before = session
                    .as_ref()
                    .map(|access| access.screen_text())
                    .unwrap_or_default();
                self.phase = Some(Phase::Running {
                    call,
                    turn,
                    waiting: Waiting::Terminal {
                        before,
                        until_ms: self.now_ms + timeout,
                    },
                });
            }
            TOOL_WEB_FETCH => {
                let url = arguments["url"].as_str().unwrap_or_default().to_owned();
                let max = arguments["maxChars"].as_u64();
                self.spawn_worker(call, turn, move || web::web_fetch(&url, max));
            }
            TOOL_WEB_SEARCH => {
                let query = arguments["query"].as_str().unwrap_or_default().to_owned();
                let provider = arguments["provider"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| self.tools.web_provider.clone());
                let key = arguments["apiKey"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| self.tools.web_key.clone());
                let limit = arguments["limit"].as_u64();
                self.spawn_worker(call, turn, move || {
                    web::web_search(&query, provider.as_deref(), key.as_deref(), limit)
                });
            }
            TOOL_RAG_SEARCH => {
                let query = arguments["query"].as_str().unwrap_or_default().to_owned();
                let limit = arguments["limit"].as_u64().map_or(5, |value| value as usize);
                let folders = self.tools.rag_folders.clone();
                let embed = self.tools.embed.clone();
                self.spawn_worker(call, turn, move || {
                    let index = rag::cached(&folders, embed.as_ref())?;
                    let query_vector = match &embed {
                        Some(config) => match embeddings(config, std::slice::from_ref(&query)) {
                            Ok(mut vectors) if !vectors.is_empty() => Some(vectors.remove(0)),
                            Ok(_) => None,
                            Err(error) => return Err(error),
                        },
                        None => None,
                    };
                    let hits = rag::search(&index, &query, query_vector.as_deref(), limit);
                    Ok(rag::format_hits(&query, &hits))
                });
            }
            _ => {
                let label = call_label(&call);
                match tools::run_local(name, arguments, self.now_ms) {
                    Ok(output) => self.settle(call, turn, label, output, false, session),
                    Err(error) => self.settle(call, turn, label, error, true, session),
                }
            }
        }
    }

    /// Run a shell command for `run_command`.
    fn exec_command(
        &mut self,
        call: ApiToolCall,
        command: String,
        sudo_stdin: Option<String>,
        turn: usize,
        session: Session<'_, '_>,
    ) {
        let Some(access) = session.as_mut() else {
            self.settle(
                call,
                turn,
                format!("$ {command}"),
                "The session is not connected.".to_owned(),
                true,
                session,
            );
            return;
        };
        self.last_command = Some(command.clone());
        self.sudo_waiting = false;
        match &sudo_stdin {
            Some(password) => {
                let wrapped = wrap_command_with_sudo(&self.cwd, &command);
                access.exec_input(EXEC_KEY, &wrapped, password);
            }
            None => {
                let wrapped = wrap_command(&self.cwd, &command);
                access.exec(EXEC_KEY, &wrapped);
            }
        }
        self.phase = Some(Phase::Running {
            call,
            turn,
            waiting: Waiting::Command {
                key: EXEC_KEY,
                then: CommandThen::Return,
            },
        });
    }

    /// Run a probe command on the session.
    fn run_probe(
        &mut self,
        call: ApiToolCall,
        turn: usize,
        then: CommandThen,
        command: String,
        session: Session<'_, '_>,
    ) {
        let label = call_label(&call);
        let Some(access) = session.as_mut() else {
            self.settle(
                call,
                turn,
                label,
                "The session is not connected.".to_owned(),
                true,
                session,
            );
            return;
        };
        access.exec(PROBE_KEY, &command);
        self.phase = Some(Phase::Running {
            call,
            turn,
            waiting: Waiting::Command {
                key: PROBE_KEY,
                then,
            },
        });
    }

    /// Start a remote read.
    fn start_read(
        &mut self,
        call: ApiToolCall,
        turn: usize,
        path: String,
        max: u64,
        then: ReadThen,
        session: Session<'_, '_>,
    ) {
        let Some(access) = session.as_mut() else {
            self.settle(
                call,
                turn,
                format!("read {path}"),
                "The session is not connected.".to_owned(),
                true,
                session,
            );
            return;
        };
        access.read(&path, max);
        self.phase = Some(Phase::Running {
            call,
            turn,
            waiting: Waiting::Read { path, max, then },
        });
    }

    /// Run `job` on a worker thread and wait for its answer.
    fn spawn_worker(
        &mut self,
        call: ApiToolCall,
        turn: usize,
        job: impl FnOnce() -> ToolResult + Send + 'static,
    ) {
        let (sender, events) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(job());
        });
        self.phase = Some(Phase::Running {
            call,
            turn,
            waiting: Waiting::Worker { events },
        });
    }

    /// Answer a pending approval.
    pub fn approve(&mut self, allow: bool, remember: Option<usize>, session: Session<'_, '_>) {
        let Some(Phase::Approving {
            call,
            command,
            scopes,
        }) = self.phase.take()
        else {
            return;
        };
        let turn = self.turn_for(&call);
        if allow {
            if let Some(index) = remember
                && let Some(scope) = scopes.get(index)
            {
                self.session_allow.push(scope.pattern.clone());
            }
            self.exec_command(call, command, None, turn, session);
        } else {
            if let Some(index) = remember
                && let Some(scope) = scopes.get(index)
            {
                self.session_deny.push(scope.pattern.clone());
            }
            self.settle(
                call,
                turn,
                format!("$ {command}"),
                format!("The user refused to run `{command}`."),
                true,
                session,
            );
        }
    }

    /// Re-run the last command with a `sudo` password on stdin.
    pub fn retry_with_sudo(&mut self, password: &str, session: Session<'_, '_>) {
        let Some(command) = self.last_command.clone() else {
            return;
        };
        let password = password.trim().to_owned();
        if password.is_empty() {
            return;
        }
        self.sudo_waiting = false;
        self.error = None;
        // The last tool turn answers again, so reuse it.
        let turn = self
            .turns
            .iter()
            .rposition(|turn| matches!(turn, Turn::Tool { .. }))
            .unwrap_or_else(|| {
                self.turns.push(Turn::Tool {
                    name: TOOL_RUN_COMMAND.to_owned(),
                    label: format!("$ {command}"),
                    output: String::new(),
                    failed: false,
                });
                self.turns.len() - 1
            });
        let call = ApiToolCall {
            id: format!("sudo-{}", self.now_ms),
            name: TOOL_RUN_COMMAND.to_owned(),
            arguments: serde_json::json!({ "command": command, "sudoStdin": password }).to_string(),
        };
        // The retry is a tool call of its own, so the history stays valid.
        self.history.push(ApiMessage::Assistant {
            content: String::new(),
            tool_calls: vec![call.clone()],
        });
        self.exec_command(call, command, Some(password), turn, session);
    }

    /// Allow every command for the rest of this session, then run this one.
    pub fn approve_everything(&mut self, session: Session<'_, '_>) {
        let Some(Phase::Approving { call, command, .. }) = self.phase.take() else {
            return;
        };
        self.session_allow.push(ALLOW_ALL_PATTERN.to_owned());
        let turn = self.turn_for(&call);
        self.exec_command(call, command, None, turn, session);
    }

    /// The transcript line belonging to a call with no answer yet.
    fn turn_for(&self, call: &ApiToolCall) -> usize {
        let wanted = call_label(call);
        self.turns
            .iter()
            .rposition(|turn| {
                matches!(turn, Turn::Tool { label, output, .. } if *label == wanted && output.is_empty())
            })
            .unwrap_or_else(|| self.turns.len().saturating_sub(1))
    }

    /// Answer the agent's question.
    pub fn answer(&mut self, text: &str, session: Session<'_, '_>) {
        let Some(Phase::Asking { call, .. }) = self.phase.take() else {
            return;
        };
        let answer = text.trim().to_owned();
        if let Some(Turn::Question { answer: slot, .. }) = self
            .turns
            .iter_mut()
            .rev()
            .find(|turn| matches!(turn, Turn::Question { answer: None, .. }))
        {
            *slot = Some(answer.clone());
        }
        let turn = self.turn_for(&call);
        self.settle(call, turn, "ask_user".to_owned(), answer, false, session);
    }

    /// Stop the reply that is streaming in.
    pub fn stop(&mut self) {
        if let Some(Phase::Streaming { turn, .. }) = self.phase.take()
            && let Some(Turn::Assistant { stopped, .. }) = self.turns.get_mut(turn)
        {
            *stopped = true;
        }
    }

    /// Take the "model owes an answer" flag.
    pub fn take_needs_reply(&mut self) -> bool {
        std::mem::take(&mut self.needs_reply)
    }

    /// The transcript as plain text, for summarising.
    pub fn transcript_text(&self) -> String {
        let mut out = String::new();
        for turn in &self.turns {
            match turn {
                Turn::User(text) => out.push_str(&format!("User: {text}\n")),
                Turn::Assistant { text, .. } => out.push_str(&format!("Assistant: {text}\n")),
                Turn::Tool { label, output, .. } => {
                    let body: String = output.chars().take(2_000).collect();
                    out.push_str(&format!("Tool {label}: {body}\n"));
                }
                Turn::Denied(reason) => out.push_str(&format!("Denied: {reason}\n")),
                Turn::Question { text, answer } => out.push_str(&format!(
                    "Question: {text}\nAnswer: {}\n",
                    answer.as_deref().unwrap_or("(no answer)")
                )),
                Turn::Error(text) => out.push_str(&format!("Error: {text}\n")),
                Turn::Notice(text) => out.push_str(&format!("Note: {text}\n")),
            }
        }
        out
    }
}

/// The label shown for a tool call.
pub fn call_label(call: &ApiToolCall) -> String {
    let arguments: Value = serde_json::from_str(&call.arguments).unwrap_or(Value::Null);
    match call.name.as_str() {
        TOOL_RUN_COMMAND => format!("$ {}", arguments["command"].as_str().unwrap_or_default()),
        TOOL_REMOTE_READ | TOOL_LOCAL_READ | TOOL_DEV_VIEW => {
            format!("read {}", arguments["path"].as_str().unwrap_or_default())
        }
        TOOL_REMOTE_LIST | TOOL_LOCAL_LIST | TOOL_DEV_LIST => {
            format!("list {}", arguments["path"].as_str().unwrap_or_default())
        }
        TOOL_REMOTE_WRITE | TOOL_LOCAL_WRITE | TOOL_DEV_CREATE => {
            format!("write {}", arguments["path"].as_str().unwrap_or_default())
        }
        TOOL_REMOTE_EDIT | TOOL_LOCAL_EDIT | TOOL_DEV_EDIT => {
            format!("edit {}", arguments["path"].as_str().unwrap_or_default())
        }
        TOOL_REMOTE_DELETE => format!("delete {}", arguments["path"].as_str().unwrap_or_default()),
        TOOL_DEV_GREP => format!("grep {}", arguments["pattern"].as_str().unwrap_or_default()),
        TOOL_DEV_FIND => format!("find {}", arguments["namePattern"].as_str().unwrap_or_default()),
        TOOL_DEV_DIFF => format!("diff {}", arguments["path"].as_str().unwrap_or_default()),
        TOOL_WEB_FETCH => format!("fetch {}", arguments["url"].as_str().unwrap_or_default()),
        TOOL_WEB_SEARCH => format!("search {}", arguments["query"].as_str().unwrap_or_default()),
        TOOL_RAG_SEARCH => format!("rag {}", arguments["query"].as_str().unwrap_or_default()),
        TOOL_CURRENT_TIME => "time".to_owned(),
        TOOL_TERMINAL_READ => "terminal read".to_owned(),
        TOOL_TERMINAL_WRITE => format!(
            "terminal type {}",
            arguments["text"].as_str().unwrap_or_default()
        ),
        TOOL_TERMINAL_KEYS => format!(
            "terminal keys {}",
            arguments["keys"].as_str().unwrap_or_default()
        ),
        TOOL_TERMINAL_WAIT => "terminal wait".to_owned(),
        TOOL_SYSTEM_OVERVIEW => "system overview".to_owned(),
        TOOL_SYSTEM_PROCESSES => "processes".to_owned(),
        TOOL_SYSTEM_DISK => "disk usage".to_owned(),
        TOOL_SYSTEM_MEMORY => "memory report".to_owned(),
        TOOL_ASK_USER => "question".to_owned(),
        other => other.to_owned(),
    }
}

/// Quote a path for `sh`.
pub fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Join `name` onto `cwd` unless it is already absolute.
pub fn relative_remote(cwd: &str, name: &str) -> String {
    let name = name.trim();
    if name.starts_with('/') {
        name.to_owned()
    } else if name.is_empty() {
        cwd.to_owned()
    } else {
        tools::join_path(cwd, name)
    }
}

/// Wrap a command so its exit status and working directory come back too.
pub fn wrap_command(cwd: &str, command: &str) -> String {
    let status = "__WASSH_STATUS";
    let pwd = "__WASSH_PWD";
    [
        format!("cd {} >/dev/null 2>&1 || true", shell_quote(cwd)),
        command.to_owned(),
        "__wassh_status=$?".to_owned(),
        format!(r#"printf '\n{status}:%s__\n' "$__wassh_status""#),
        format!(r#"printf '{pwd}:%s__\n' "$(pwd 2>/dev/null)""#),
    ]
    .join("\n")
}

/// Wrap a command that needs `sudo`, reading the password from stdin.
///
/// Port of the reference's sudo preamble: the password is written to the channel
/// and read with `IFS= read -r`, so it never reaches the command line.
pub fn wrap_command_with_sudo(cwd: &str, command: &str) -> String {
    let status = "__WASSH_STATUS";
    let pwd = "__WASSH_PWD";
    [
        format!("cd {} >/dev/null 2>&1 || true", shell_quote(cwd)),
        "IFS= read -r __wassh_sudo_pw || true".to_owned(),
        "if [ -n \"${__wassh_sudo_pw}\" ]; then".to_owned(),
        "  printf '%s\\n' \"${__wassh_sudo_pw}\" | sudo -S -p '' -v >/dev/null 2>&1".to_owned(),
        "  __wassh_sudo_auth=$?".to_owned(),
        "else".to_owned(),
        "  sudo -n -v >/dev/null 2>&1".to_owned(),
        "  __wassh_sudo_auth=$?".to_owned(),
        "fi".to_owned(),
        "unset __wassh_sudo_pw".to_owned(),
        "if [ \"${__wassh_sudo_auth}\" -ne 0 ]; then".to_owned(),
        format!("  printf '%s\\n' '{SUDO_AUTH_FAILED_MARKER}'"),
        "  __wassh_status=1".to_owned(),
        "else".to_owned(),
        format!("  {command}"),
        "  __wassh_status=$?".to_owned(),
        "fi".to_owned(),
        format!(r#"printf '\n{status}:%s__\n' "$__wassh_status""#),
        format!(r#"printf '{pwd}:%s__\n' "$(pwd 2>/dev/null)""#),
    ]
    .join("\n")
}

/// Undo [`wrap_command`]: the visible output, exit status and directory.
pub fn unwrap_output(raw: &str) -> (String, Option<i32>, Option<String>) {
    let mut exit_code = None;
    let mut directory = None;
    let mut visible = String::with_capacity(raw.len());
    for line in raw.split('\n') {
        let trimmed = line.trim_end_matches('\r');
        if let Some(rest) = trimmed.strip_prefix("__WASSH_STATUS:") {
            exit_code = rest.trim_end_matches("__").trim().parse().ok();
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("__WASSH_PWD:") {
            directory = Some(rest.trim_end_matches("__").to_owned());
            continue;
        }
        visible.push_str(trimmed);
        visible.push('\n');
    }
    while visible.ends_with('\n') {
        visible.pop();
    }
    (visible, exit_code, directory)
}

/// Keep only the tail a command can still have its markers in.
fn tail_of(raw: &str) -> String {
    let keep = MAX_OUTPUT_CHARS + MARKER_TAIL_CHARS;
    let count = raw.chars().count();
    if count > keep {
        raw.chars().skip(count - keep).collect()
    } else {
        raw.to_owned()
    }
}

/// Cut the visible output to what the model may read.
fn clamp_output(text: String) -> String {
    if text.chars().count() <= MAX_OUTPUT_CHARS {
        return text;
    }
    text.chars().take(MAX_OUTPUT_CHARS).collect()
}

/// Apply an edit and describe it.
fn replace_and_report(text: &str, old: &str, new: &str) -> Result<(String, String), String> {
    let updated = tools::replace_once(text, old, new)?;
    Ok((updated.clone(), tools::diff_texts(text, &updated, 3)))
}

/// What the model is told about one command.
pub fn command_result_text(output: &str, exit_code: Option<i32>, truncated: bool) -> String {
    let mut text = String::new();
    if output.is_empty() {
        text.push_str("(no output)\n");
    } else {
        text.push_str(output);
        if !text.ends_with('\n') {
            text.push('\n');
        }
    }
    text.push_str(&format!(
        "[exit code: {}]",
        exit_code.map_or_else(|| "unknown".to_owned(), |code| code.to_string())
    ));
    if truncated {
        text.push_str("\n[output truncated]");
    }
    text
}

/// The transcript as stored JSON.
pub fn turns_to_json(turns: &[Turn]) -> Vec<Value> {
    turns
        .iter()
        .map(|turn| match turn {
            Turn::User(text) => serde_json::json!({ "role": "user", "text": text }),
            Turn::Assistant { text, stopped } => {
                serde_json::json!({ "role": "assistant", "text": text, "stopped": stopped })
            }
            Turn::Tool {
                name,
                label,
                output,
                failed,
            } => serde_json::json!({
                "role": "tool",
                "name": name,
                "label": label,
                "output": output,
                "failed": failed
            }),
            Turn::Denied(reason) => serde_json::json!({ "role": "denied", "text": reason }),
            Turn::Question { text, answer } => {
                serde_json::json!({ "role": "question", "text": text, "answer": answer })
            }
            Turn::Error(text) => serde_json::json!({ "role": "error", "text": text }),
            Turn::Notice(text) => serde_json::json!({ "role": "notice", "text": text }),
        })
        .collect()
}

/// Read a stored transcript back.
pub fn turns_from_json(items: &[Value]) -> Vec<Turn> {
    items
        .iter()
        .filter_map(|item| {
            let text = || item["text"].as_str().unwrap_or_default().to_owned();
            match item["role"].as_str()? {
                "user" => Some(Turn::User(text())),
                "assistant" => Some(Turn::Assistant {
                    text: text(),
                    stopped: item["stopped"].as_bool().unwrap_or(false),
                }),
                "tool" => Some(Turn::Tool {
                    name: item["name"].as_str().unwrap_or_default().to_owned(),
                    label: item["label"].as_str().unwrap_or_default().to_owned(),
                    output: item["output"].as_str().unwrap_or_default().to_owned(),
                    failed: item["failed"].as_bool().unwrap_or(false),
                }),
                "denied" => Some(Turn::Denied(text())),
                "question" => Some(Turn::Question {
                    text: text(),
                    answer: item["answer"].as_str().map(str::to_owned),
                }),
                "error" => Some(Turn::Error(text())),
                "notice" => Some(Turn::Notice(text())),
                _ => None,
            }
        })
        .collect()
}

/// The provider history as stored JSON.
pub fn history_to_json(messages: &[ApiMessage]) -> Vec<Value> {
    messages
        .iter()
        .map(|message| match message {
            ApiMessage::User { content } => serde_json::json!({ "role": "user", "content": content }),
            ApiMessage::Assistant {
                content,
                tool_calls,
            } => serde_json::json!({
                "role": "assistant",
                "content": content,
                "toolCalls": tool_calls
                    .iter()
                    .map(|call| serde_json::json!({
                        "id": call.id,
                        "name": call.name,
                        "arguments": call.arguments
                    }))
                    .collect::<Vec<_>>()
            }),
            ApiMessage::Tool {
                tool_call_id,
                content,
            } => serde_json::json!({
                "role": "tool",
                "toolCallId": tool_call_id,
                "content": content
            }),
        })
        .collect()
}

/// Read a stored provider history back.
pub fn history_from_json(items: &[Value]) -> Vec<ApiMessage> {
    items
        .iter()
        .filter_map(|item| match item["role"].as_str()? {
            "user" => Some(ApiMessage::User {
                content: item["content"].as_str().unwrap_or_default().to_owned(),
            }),
            "assistant" => Some(ApiMessage::Assistant {
                content: item["content"].as_str().unwrap_or_default().to_owned(),
                tool_calls: item["toolCalls"]
                    .as_array()
                    .map(|calls| {
                        calls
                            .iter()
                            .map(|call| ApiToolCall {
                                id: call["id"].as_str().unwrap_or_default().to_owned(),
                                name: call["name"].as_str().unwrap_or_default().to_owned(),
                                arguments: call["arguments"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .to_owned(),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            }),
            "tool" => Some(ApiMessage::Tool {
                tool_call_id: item["toolCallId"].as_str().unwrap_or_default().to_owned(),
                content: item["content"].as_str().unwrap_or_default().to_owned(),
            }),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wrapped_command_reports_status_and_directory() {
        let wrapped = wrap_command("/srv/app", "ls -la");
        assert!(wrapped.starts_with("cd '/srv/app' >/dev/null 2>&1 || true\nls -la\n"));
        assert!(wrapped.contains("__WASSH_STATUS:%s__"));
        assert!(wrapped.contains("__WASSH_PWD:%s__"));
    }

    #[test]
    fn unwrapping_keeps_the_output_and_reads_the_markers() {
        let raw = "total 4\ndrwxr-xr-x 2 root root\n__WASSH_STATUS:0__\n__WASSH_PWD:/srv/app__\n";
        let (output, code, directory) = unwrap_output(raw);
        assert_eq!(output, "total 4\ndrwxr-xr-x 2 root root");
        assert_eq!(code, Some(0));
        assert_eq!(directory.as_deref(), Some("/srv/app"));
    }

    #[test]
    fn the_tool_result_carries_the_exit_code() {
        assert_eq!(command_result_text("ok", Some(0), false), "ok\n[exit code: 0]");
        assert_eq!(command_result_text("", Some(2), false), "(no output)\n[exit code: 2]");
        assert!(command_result_text("big", None, true).contains("[output truncated]"));
    }

    #[test]
    fn shell_quoting_survives_a_quote_in_the_path() {
        assert_eq!(shell_quote("/srv/it's here"), r"'/srv/it'\''s here'");
    }

    #[test]
    fn remote_paths_resolve_against_the_working_directory() {
        assert_eq!(relative_remote("/srv/app", "/etc/passwd"), "/etc/passwd");
        assert_eq!(relative_remote("/srv/app", "notes.txt"), "/srv/app/notes.txt");
        assert_eq!(relative_remote("/srv/app", ""), "/srv/app");
        assert_eq!(relative_remote("/", "etc"), "/etc");
    }

    #[test]
    fn tool_labels_read_like_the_reference() {
        let call = |name: &str, arguments: &str| ApiToolCall {
            id: "c1".to_owned(),
            name: name.to_owned(),
            arguments: arguments.to_owned(),
        };
        assert_eq!(call_label(&call(TOOL_RUN_COMMAND, r#"{"command":"uptime"}"#)), "$ uptime");
        assert_eq!(
            call_label(&call(TOOL_REMOTE_READ, r#"{"path":"/etc/hosts"}"#)),
            "read /etc/hosts"
        );
        assert_eq!(call_label(&call(TOOL_CURRENT_TIME, "{}")), "time");
        assert_eq!(call_label(&call(TOOL_WEB_SEARCH, r#"{"query":"rust"}"#)), "search rust");
    }

    #[test]
    fn the_transcript_round_trips_through_json() {
        let turns = vec![
            Turn::User("hello".to_owned()),
            Turn::Assistant {
                text: "hi".to_owned(),
                stopped: false,
            },
            Turn::Tool {
                name: TOOL_RUN_COMMAND.to_owned(),
                label: "$ uptime".to_owned(),
                output: "up 3 days".to_owned(),
                failed: false,
            },
            Turn::Question {
                text: "which file?".to_owned(),
                answer: Some("a.txt".to_owned()),
            },
            Turn::Notice("summary saved".to_owned()),
        ];
        let json = turns_to_json(&turns);
        assert_eq!(turns_from_json(&json), turns);
    }

    #[test]
    fn edits_are_applied_and_described() {
        let (updated, diff) = replace_and_report("a\nb\n", "b", "B").unwrap();
        assert_eq!(updated, "a\nB\n");
        assert!(diff.contains("+   1 B"), "{diff}");
        assert!(replace_and_report("a\n", "zz", "y").is_err());
    }

    #[test]
    fn an_unknown_tool_is_refused_without_touching_the_session() {
        let mut agent = Agent::new();
        let mut session: Option<&mut dyn SessionAccess> = None;
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: "no_such_tool".to_owned(),
                arguments: "{}".to_owned(),
            },
            &mut session,
        );
        assert!(matches!(&agent.turns[0], Turn::Tool { failed: true, .. }));
        assert!(agent.needs_reply);
        assert!(matches!(
            agent.history.last(),
            Some(ApiMessage::Tool { content, .. }) if content.contains("not available")
        ));
    }

    #[test]
    fn a_local_tool_answers_immediately() {
        let mut agent = Agent::new();
        agent.now_ms = 1_700_000_000_000;
        let mut session: Option<&mut dyn SessionAccess> = None;
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: TOOL_CURRENT_TIME.to_owned(),
                arguments: "{}".to_owned(),
            },
            &mut session,
        );
        match &agent.turns[0] {
            Turn::Tool { output, failed, .. } => {
                assert!(!failed);
                assert!(output.contains("1700000000000"), "{output}");
            }
            other => panic!("expected a tool turn, got {other:?}"),
        }
        assert!(agent.needs_reply);
    }

    #[test]
    fn a_forbidden_command_is_refused_and_reported_to_the_model() {
        let mut agent = Agent::new();
        agent.app_deny = vec!["rm -rf *".to_owned()];
        let mut session: Option<&mut dyn SessionAccess> = None;
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: TOOL_RUN_COMMAND.to_owned(),
                arguments: r#"{"command":"rm -rf /"}"#.to_owned(),
            },
            &mut session,
        );
        assert!(matches!(&agent.turns[0], Turn::Tool { failed: true, .. }));
        assert!(agent.needs_reply);
    }

    #[test]
    fn a_command_outside_the_rules_waits_for_approval() {
        let mut agent = Agent::new();
        let mut session: Option<&mut dyn SessionAccess> = None;
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: TOOL_RUN_COMMAND.to_owned(),
                arguments: r#"{"command":"systemctl restart nginx"}"#.to_owned(),
            },
            &mut session,
        );
        assert!(agent.approving());
        // Without a session the denial still lets the model continue.
        agent.approve(false, None, &mut session);
        assert!(!agent.approving());
        assert!(agent.needs_reply);
        assert!(matches!(&agent.turns[0], Turn::Tool { failed: true, .. }));
    }

    #[test]
    fn ask_user_waits_for_the_answer() {
        let mut agent = Agent::new();
        let mut session: Option<&mut dyn SessionAccess> = None;
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: TOOL_ASK_USER.to_owned(),
                arguments: r#"{"text":"Which file?"}"#.to_owned(),
            },
            &mut session,
        );
        assert!(agent.asking());
        agent.answer("notes.txt", &mut session);
        assert!(!agent.asking());
        assert!(agent.turns.iter().any(|turn| matches!(
            turn,
            Turn::Question { answer: Some(answer), .. } if answer == "notes.txt"
        )));
        assert!(agent.needs_reply);
    }

    #[test]
    fn a_remote_tool_without_a_session_says_so() {
        let mut agent = Agent::new();
        let mut session: Option<&mut dyn SessionAccess> = None;
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: TOOL_REMOTE_LIST.to_owned(),
                arguments: r#"{"path":"/etc"}"#.to_owned(),
            },
            &mut session,
        );
        assert!(agent.needs_reply);
        match &agent.turns[0] {
            Turn::Tool { output, failed, .. } => {
                assert!(failed);
                assert!(output.contains("not connected"), "{output}");
            }
            other => panic!("expected a tool turn, got {other:?}"),
        }
    }


    /// A session that answers from memory, so the tool paths can be driven.
    #[derive(Default)]
    struct FakeSession {
        screen: String,
        typed: String,
        listing: Option<crate::files::SftpListing>,
        contents: std::collections::HashMap<String, String>,
        commands: Vec<(String, String)>,
        answers: std::collections::HashMap<String, crate::command::CommandOutput>,
        uploads: Vec<(String, String)>,
        transferring: bool,
        status: Option<String>,
    }

    impl crate::files::SftpAccess for FakeSession {
        fn request_listing(&mut self, _path: &str) {}
        fn refresh(&mut self, _path: &str) {}
        fn listing(&self) -> Option<&crate::files::SftpListing> {
            self.listing.as_ref()
        }
        fn is_loading(&self) -> bool {
            false
        }
        fn error(&self) -> Option<&str> {
            None
        }
        fn upload(&mut self, local: &str, remote: &str) {
            let body = std::fs::read_to_string(local).unwrap_or_default();
            self.uploads.push((remote.to_owned(), body));
            self.status = Some(format!("Uploaded {remote}"));
            self.transferring = false;
        }
        fn download(&mut self, _remote: &str, _local: &str) {}
        fn is_transferring(&self) -> bool {
            self.transferring
        }
        fn transfer_status(&self) -> Option<&str> {
            self.status.as_deref()
        }
        fn report_problem(&mut self, message: &str) {
            self.status = Some(message.to_owned());
        }
        fn resolve(&mut self, path: &str) -> Option<String> {
            Some(path.to_owned())
        }
        fn mkdir(&mut self, _path: &str) {}
        fn rename(&mut self, _from: &str, _to: &str) {}
        fn remove(&mut self, _path: &str, _is_dir: bool) {}
        fn chmod(&mut self, _path: &str, _mode: u32) {}
        fn read(&mut self, path: &str, _max_bytes: u64) -> Option<crate::files::FileContent> {
            self.contents.get(path).map(|text| crate::files::FileContent {
                data: text.as_bytes().to_vec(),
                truncated: false,
            })
        }
        fn upload_many(&mut self, _items: Vec<(String, String)>) {}
        fn download_many(&mut self, _items: Vec<(String, String)>) {}
        fn copy_many(&mut self, _items: Vec<(String, String)>, _move_items: bool) {}
        fn progress(&self) -> Option<(u64, Option<u64>)> {
            None
        }
        fn download_tree(&mut self, _items: Vec<String>, _dir: &str) {}
        fn download_zip(&mut self, _items: Vec<String>, _archive: &str) {}
        fn transfers(&self) -> Vec<crate::files::TransferRow> {
            Vec::new()
        }
        fn retry(&mut self) {}
    }

    impl crate::monitor::MonitorAccess for FakeSession {
        fn request_sample(&mut self) {}
        fn sample(&self) -> Option<&crate::monitor::Snapshot> {
            None
        }
        fn is_sampling(&self) -> bool {
            false
        }
        fn last_error(&self) -> Option<&str> {
            None
        }
    }

    impl crate::command::CommandAccess for FakeSession {
        fn exec(&mut self, key: &str, command: &str) {
            self.commands.push((key.to_owned(), command.to_owned()));
        }
        fn output(&self, key: &str) -> Option<&crate::command::CommandOutput> {
            self.answers.get(key)
        }
        fn failure(&self, _key: &str) -> Option<&str> {
            None
        }
        fn is_running(&self, _key: &str) -> bool {
            false
        }
    }

    impl crate::command::TcpAccess for FakeSession {
        fn open_forward(&mut self, _key: &str, _host: &str, _port: u16) {}

        fn forward_address(&self, _key: &str) -> Option<&str> {
            None
        }

        fn forward_error(&self, _key: &str) -> Option<&str> {
            None
        }

        fn forward_pending(&self, _key: &str) -> bool {
            false
        }
    }

    impl crate::plugin::TerminalAccess for FakeSession {
        fn screen_text(&self) -> String {
            self.screen.clone()
        }
        fn type_text(&mut self, text: &str) {
            self.typed.push_str(text);
        }
        fn is_closed(&self) -> bool {
            false
        }
    }

    /// Drive one tool call against `fake` until it settles.
    fn run_tool(agent: &mut Agent, fake: &mut FakeSession, name: &str, arguments: &str) -> String {
        let mut session: Option<&mut dyn SessionAccess> = Some(fake);
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: name.to_owned(),
                arguments: arguments.to_owned(),
            },
            &mut session,
        );
        agent.poll(&mut session);
        match agent.turns.last() {
            Some(Turn::Tool { output, .. }) => output.clone(),
            other => panic!("expected a tool turn, got {other:?}"),
        }
    }

    #[test]
    fn a_command_runs_through_the_session_and_moves_the_directory() {
        let mut agent = Agent::new();
        agent.app_allow = vec!["pwd".to_owned()];
        let mut fake = FakeSession::default();
        fake.answers.insert(
            EXEC_KEY.to_owned(),
            crate::command::CommandOutput {
                stdout: "/srv/app\n__WASSH_STATUS:0__\n__WASSH_PWD:/srv/app__\n".to_owned(),
                stderr: String::new(),
                code: Some(0),
            },
        );
        let output = run_tool(&mut agent, &mut fake, TOOL_RUN_COMMAND, r#"{"command":"pwd"}"#);
        assert_eq!(output, "/srv/app
[exit code: 0]");
        // The wrapper ran in the cwd and reported it back.
        let sent = &fake.commands[0].1;
        assert!(sent.starts_with("cd '/' "), "{sent}");
        assert_eq!(agent.cwd, "/srv/app");
        assert!(agent.needs_reply);
    }

    #[test]
    fn a_remote_read_answers_from_the_session() {
        let mut agent = Agent::new();
        let mut fake = FakeSession::default();
        fake.contents
            .insert("/etc/hosts".to_owned(), "127.0.0.1 localhost
".to_owned());
        let output = run_tool(
            &mut agent,
            &mut fake,
            TOOL_REMOTE_READ,
            r#"{"path":"/etc/hosts"}"#,
        );
        assert_eq!(output, "127.0.0.1 localhost
");
    }

    #[test]
    fn a_remote_edit_uploads_the_new_text_and_reports_the_diff() {
        let mut agent = Agent::new();
        let mut fake = FakeSession::default();
        // The edit resolves `translated.txt` against the working directory.
        fake.contents.insert("/translated.txt".to_owned(), "one\ntwo\n".to_owned());
        let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: TOOL_REMOTE_EDIT.to_owned(),
                arguments: r#"{"path":"translated.txt","oldText":"two","newText":"TWO"}"#
                    .to_owned(),
            },
            &mut session,
        );
        agent.poll(&mut session);
        let upload = fake.uploads.last().cloned();
        assert_eq!(
            upload.as_ref().map(|(path, _)| path.as_str()),
            Some("/translated.txt")
        );
        assert!(upload.as_ref().is_some_and(|(_, body)| body.contains("TWO")));
        match agent.turns.last().cloned().expect("a tool turn") {
            Turn::Tool { output, failed, .. } => {
                assert!(!failed, "{output}");
                assert!(output.contains("+   1 TWO"), "{output}");
            }
            other => panic!("expected a tool turn, got {other:?}"),
        }
    }

    #[test]
    fn a_remote_listing_is_formatted_for_the_model() {
        let mut agent = Agent::new();
        let mut fake = FakeSession {
            listing: Some(crate::files::SftpListing {
                path: "/etc".to_owned(),
                entries: vec![
                    crate::files::SftpEntry {
                        name: "nginx".to_owned(),
                        path: "/etc/nginx".to_owned(),
                        is_dir: true,
                        is_link: false,
                        size: 0,
                        modified: None,
                        permissions: None,
                    },
                    crate::files::SftpEntry {
                        name: "hosts".to_owned(),
                        path: "/etc/hosts".to_owned(),
                        is_dir: false,
                        is_link: false,
                        size: 220,
                        modified: None,
                        permissions: None,
                    },
                ],
            }),
            ..FakeSession::default()
        };
        let output = run_tool(&mut agent, &mut fake, TOOL_REMOTE_LIST, r#"{"path":"/etc"}"#);
        assert!(output.contains("dir   nginx"), "{output}");
        assert!(output.contains("file       220  hosts"), "{output}");
    }

    #[test]
    fn terminal_tools_read_and_type_on_the_real_screen() {
        let mut agent = Agent::new();
        let mut fake = FakeSession {
            screen: "$ uptime
 10:00 up 3 days".to_owned(),
            ..FakeSession::default()
        };
        let read = run_tool(&mut agent, &mut fake, TOOL_TERMINAL_READ, "{}");
        assert!(read.contains("up 3 days"), "{read}");

        let typed = run_tool(
            &mut agent,
            &mut fake,
            TOOL_TERMINAL_WRITE,
            r#"{"text":"ls","pressEnter":true}"#,
        );
        assert!(typed.contains("Typed 2 characters and Enter"), "{typed}");
        assert_eq!(fake.typed, "ls\r");

        let keys = run_tool(&mut agent, &mut fake, TOOL_TERMINAL_KEYS, r#"{"keys":"ctrl-c"}"#);
        assert!(keys.contains("Sent ctrl-c"), "{keys}");
        assert!(fake.typed.ends_with('\u{3}'));
    }

    #[test]
    fn a_wait_returns_once_the_screen_changes() {
        let mut agent = Agent::new();
        agent.now_ms = 1_000;
        let mut fake = FakeSession {
            screen: "working".to_owned(),
            ..FakeSession::default()
        };
        {
            let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
            agent.begin_tool(
                ApiToolCall {
                    id: "c1".to_owned(),
                    name: TOOL_TERMINAL_WAIT.to_owned(),
                    arguments: r#"{"timeoutMs":5000}"#.to_owned(),
                },
                &mut session,
            );
            // Nothing changed yet and the timeout has not passed.
            assert!(!agent.poll(&mut session));
        }
        fake.screen = "done".to_owned();
        {
            let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
            assert!(agent.poll(&mut session));
        }
        match agent.turns.last() {
            Some(Turn::Tool { output, .. }) => assert!(output.contains("done"), "{output}"),
            other => panic!("expected a tool turn, got {other:?}"),
        }
    }

    #[test]
    fn a_refused_sudo_password_waits_for_the_user_and_then_runs() {
        let mut agent = Agent::new();
        agent.app_allow = vec!["apt-get*".to_owned()];
        let mut fake = FakeSession::default();
        fake.answers.insert(
            EXEC_KEY.to_owned(),
            crate::command::CommandOutput {
                stdout: format!("{SUDO_AUTH_FAILED_MARKER}\n__WASSH_STATUS:1__\n__WASSH_PWD:/__\n"),
                stderr: String::new(),
                code: Some(1),
            },
        );
        run_tool(&mut agent, &mut fake, TOOL_RUN_COMMAND, r#"{"command":"apt-get update"}"#);
        assert!(agent.sudo_waiting, "a refused password must be the user's to fix");
        assert!(!agent.needs_reply, "the model must not be asked again yet");
        assert_eq!(agent.last_command.as_deref(), Some("apt-get update"));

        // Answering with the password re-runs it through `sudo -S`.
        fake.answers.insert(
            EXEC_KEY.to_owned(),
            crate::command::CommandOutput {
                stdout: "Reading package lists...\n__WASSH_STATUS:0__\n__WASSH_PWD:/__\n".to_owned(),
                stderr: String::new(),
                code: Some(0),
            },
        );
        let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
        agent.retry_with_sudo("hunter2", &mut session);
        assert!(!agent.sudo_waiting);
        let sent = &fake.commands.last().expect("a second command").1;
        assert!(sent.contains("sudo -S -p '' -v"), "{sent}");
        assert!(!sent.contains("hunter2"), "the password must not reach the command line");
    }

    #[test]
    fn a_sudo_command_reads_its_password_from_stdin() {
        let wrapped = wrap_command_with_sudo("/srv", "apt-get update");
        assert!(wrapped.contains("IFS= read -r __wassh_sudo_pw"));
        assert!(wrapped.contains("sudo -S -p '' -v"));
        assert!(wrapped.contains("apt-get update"));
        assert!(wrapped.contains(SUDO_AUTH_FAILED_MARKER));
        // The password itself is never part of the command line.
        assert!(!wrapped.contains("hunter2"));
    }

    #[test]
    fn the_summary_prompt_carries_the_transcript() {
        let mut agent = Agent::new();
        agent.turns.push(Turn::User("restart nginx".to_owned()));
        agent.turns.push(Turn::Assistant {
            text: "done".to_owned(),
            stopped: false,
        });
        let text = agent.transcript_text();
        assert!(text.contains("User: restart nginx"));
        assert!(text.contains("Assistant: done"));
        assert!(SUMMARY_SYSTEM_PROMPT.contains("Goal"));
    }
}
