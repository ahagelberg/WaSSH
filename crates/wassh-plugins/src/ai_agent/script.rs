//! The sandbox a `run_script` call executes in.
//!
//! The model writes a short JavaScript program instead of one tool call per round: the
//! script calls the conversation's own tools, filters and joins what they return, and hands
//! back only the compact result. That keeps the prompt small and the number of rounds low.
//!
//! Each run gets a fresh QuickJS runtime on its own thread. The script sees the standard
//! JavaScript built-ins, a `console`, and `tools`; every `tools.<name>(args)` suspends the
//! script until this crate answers with the tool's outcome, so nothing else reaches the
//! host. One call is in flight at a time, which is all the agent's single waiting slot
//! allows: a script may `Promise.all` a batch, but the calls still run in order. A call the
//! agent does not answer within [`MAX_CALL_MS`] is failed by the agent, so a command that
//! hangs rejects the promise instead of stalling the run forever.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Instant;

use rquickjs::{Context, Function, Runtime};

/// Longest a script may execute, not counting the time it spends waiting on a tool call.
pub const MAX_EXEC_MS: u64 = 20_000;
/// Longest a script waits on one tool call before it is failed with a timeout.
pub const MAX_CALL_MS: u64 = 60_000;
/// Longest a script may execute when the user grants it more time.
pub const MAX_TIMEOUT_MS: u64 = 600_000;
/// Memory the runtime may use.
pub const MAX_HEAP_BYTES: usize = 64 * 1024 * 1024;
/// Tool calls one run may make.
pub const MAX_CALLS: usize = 100;
/// Characters the result keeps before it is cut with a marker.
pub const MAX_RESULT_CHARS: usize = 32_000;
/// Characters console output keeps for the transcript; never sent to the model.
pub const MAX_CONSOLE_CHARS: usize = 8_000;
/// Stack the engine may use.
const MAX_STACK_BYTES: usize = 1024 * 1024;
/// How long a test waits for one event from a sandbox.
#[cfg(test)]
const TEST_WAIT_MS: u64 = 5_000;
/// Budget the tests give a runaway loop, so they do not wait the full [`MAX_EXEC_MS`].
#[cfg(test)]
const TEST_EXEC_MS: u64 = 200;

/// A call the script made and is waiting on.
#[derive(Clone, Debug)]
pub struct ScriptCall {
    /// Id the script waits for.
    pub id: u64,
    /// Tool name.
    pub name: String,
    /// Arguments as the script wrote them.
    pub arguments: String,
}

/// What the sandbox tells the agent.
#[derive(Clone, Debug)]
pub enum ScriptEvent {
    /// The script wants this tool run.
    Call(ScriptCall),
    /// The script stopped: its return value, or what went wrong, plus its console output.
    Finished {
        /// `Ok` with the JSON of the return value, `Err` with what went wrong.
        result: Result<String, String>,
        /// What the script logged, for the transcript.
        console: String,
    },
}

/// The agent's answer to one [`ScriptEvent::Call`].
#[derive(Clone, Debug)]
pub struct ScriptReply {
    /// Call being answered.
    pub id: u64,
    /// Whether the tool succeeded.
    pub ok: bool,
    /// The tool's JSON payload, or the error message.
    pub text: String,
}

/// The channels and flags of one running sandbox.
pub struct ScriptJob {
    /// What the script wants; drained by the agent each frame.
    pub events: mpsc::Receiver<ScriptEvent>,
    /// Answers to send back.
    replies: mpsc::Sender<ScriptReply>,
    /// Set when the agent no longer wants the result.
    stop: Arc<AtomicBool>,
}

impl ScriptJob {
    /// Answer a call the script is waiting on.
    pub fn reply(&self, reply: ScriptReply) {
        let _ = self.replies.send(reply);
    }

    /// Ask the sandbox to stop; a script stuck in a loop is interrupted by the engine.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for ScriptJob {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Time spent executing, which excludes the waits for the host's answers.
#[derive(Default)]
struct Clock {
    /// Milliseconds spent running JavaScript.
    spent: AtomicU64,
    /// When the current stretch started, in milliseconds of a monotonic clock.
    running_since: AtomicU64,
}

impl Clock {
    /// A clock with a stretch of execution already running.
    ///
    /// The interrupt handler reads it from the moment the runtime exists, before the
    /// prelude is evaluated: a clock that starts at zero would report the process uptime
    /// as time spent and interrupt every run once the app is older than the budget.
    fn new() -> Arc<Self> {
        let clock = Self::default();
        clock.resume();
        Arc::new(clock)
    }

    /// Mark the start of a stretch of execution.
    fn resume(&self) {
        self.running_since.store(monotonic_ms(), Ordering::SeqCst);
    }

    /// Mark the end of a stretch of execution.
    fn pause(&self) {
        let now = monotonic_ms();
        let start = self.running_since.swap(now, Ordering::SeqCst);
        self.spent
            .fetch_add(now.saturating_sub(start), Ordering::SeqCst);
    }

    /// Milliseconds executed so far, including the stretch in progress.
    fn used_ms(&self) -> u64 {
        let start = self.running_since.load(Ordering::SeqCst);
        self.spent
            .load(Ordering::SeqCst)
            .saturating_add(monotonic_ms().saturating_sub(start))
    }
}

fn monotonic_ms() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// What the JavaScript callbacks write into.
#[derive(Default)]
struct Bridge {
    /// Calls the script made and is waiting on.
    calls: Mutex<VecDeque<(u64, String, String)>>,
    /// Set once the script's promise settles.
    result: Mutex<Option<Result<String, String>>>,
    /// Everything the script logged.
    console: Mutex<String>,
}

/// The model's program, wrapped so its return value becomes the result.
fn source(code: &str) -> String {
    format!(
        "{PRELUDE}\n(async function () {{\n{code}\n}})().then(\n\
           function (value) {{\n    __wassh_result(true, globalThis.__wassh_safe(value));\n  }},\n\
           function (error) {{\n    __wassh_result(false, globalThis.__wassh_error(error));\n  }}\n);\n"
    )
}

/// Everything a script can see besides the language itself.
const PRELUDE: &str = r#"
(function () {
  const pending = [];
  let counter = 0;
  function safe(value) {
    const seen = new WeakSet();
    try {
      const text = JSON.stringify(value, function (key, item) {
        if (typeof item === 'bigint') return item.toString();
        if (typeof item === 'function' || typeof item === 'symbol') return null;
        if (item instanceof Error) return String(item);
        if (item && typeof item === 'object') {
          if (seen.has(item)) return '[circular]';
          seen.add(item);
        }
        return item;
      });
      return text === undefined ? 'null' : text;
    } catch (error) {
      return JSON.stringify(String(value));
    }
  }
  function errorText(error) {
    const head = String(error);
    if (error && typeof error === 'object' && typeof error.stack === 'string') {
      return head + '\n' + error.stack;
    }
    return head;
  }
  function call(name, args) {
    return new Promise(function (resolve, reject) {
      const id = ++counter;
      pending.push({ id: id, resolve: resolve, reject: reject });
      __wassh_call(id, String(name), JSON.stringify(args === undefined ? {} : args));
    });
  }
  globalThis.tools = new Proxy({}, {
    get: function (target, name) {
      if (typeof name !== 'string') return undefined;
      return function (args) { return call(name, args); };
    }
  });
  globalThis.console = {};
  ['log', 'info', 'warn', 'error', 'debug'].forEach(function (level) {
    globalThis.console[level] = function () {
      const parts = [];
      for (let i = 0; i < arguments.length; i++) {
        const item = arguments[i];
        parts.push(typeof item === 'string' ? item : safe(item));
      }
      __wassh_console(level, parts.join(' '));
    };
  });
  globalThis.__wassh_safe = safe;
  globalThis.__wassh_error = errorText;
  globalThis.__wassh_settle = function (id, ok, text) {
    for (let i = 0; i < pending.length; i++) {
      if (pending[i].id !== id) continue;
      const entry = pending.splice(i, 1)[0];
      if (ok) {
        let value = null;
        try { value = JSON.parse(text); } catch (error) { value = { ok: true, text: text }; }
        entry.resolve(value);
      } else {
        entry.reject(new Error(text));
      }
      return;
    }
  };
})();
"#;

/// Start a script in its own sandbox.
///
/// The returned job is driven by the caller: every [`ScriptEvent::Call`] must be answered
/// with [`ScriptJob::reply`], or the script stays suspended until the job is dropped.
pub fn spawn(code: &str) -> ScriptJob {
    spawn_with_budget(code, MAX_EXEC_MS)
}

pub fn spawn_with_budget(code: &str, budget_ms: u64) -> ScriptJob {
    let (event_tx, event_rx) = mpsc::channel();
    let (reply_tx, reply_rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let source = source(code);
    let report = event_tx.clone();
    let started = std::thread::Builder::new()
        .name("wassh-script".to_owned())
        .spawn(move || {
            let (result, console) = run(&source, &event_tx, &reply_rx, &thread_stop, budget_ms);
            let _ = event_tx.send(ScriptEvent::Finished { result, console });
        });
    if started.is_err() {
        let _ = report.send(ScriptEvent::Finished {
            result: Err("The sandbox could not be started.".to_owned()),
            console: String::new(),
        });
    }
    ScriptJob {
        events: event_rx,
        replies: reply_tx,
        stop,
    }
}

/// Run one script to its end on this thread, answering with its result and console output.
fn run(
    source: &str,
    events: &mpsc::Sender<ScriptEvent>,
    replies: &mpsc::Receiver<ScriptReply>,
    stop: &Arc<AtomicBool>,
    budget_ms: u64,
) -> (Result<String, String>, String) {
    let runtime = match Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            return (
                Err(format!("The sandbox did not start: {error}")),
                String::new(),
            );
        }
    };
    runtime.set_memory_limit(MAX_HEAP_BYTES);
    runtime.set_max_stack_size(MAX_STACK_BYTES);
    let clock = Clock::new();
    let interrupt_clock = Arc::clone(&clock);
    let interrupt_stop = Arc::clone(stop);
    runtime.set_interrupt_handler(Some(Box::new(move || {
        interrupt_stop.load(Ordering::Relaxed) || interrupt_clock.used_ms() > budget_ms
    })));

    let context = match Context::full(&runtime) {
        Ok(context) => context,
        Err(error) => {
            return (
                Err(format!("The sandbox did not start: {error}")),
                String::new(),
            );
        }
    };
    let bridge = Arc::new(Bridge::default());
    if let Err(problem) = install(&context, &bridge) {
        clock.pause();
        return (
            Err(cut_off(stop, &clock, budget_ms, problem)),
            String::new(),
        );
    }

    let started = context.with(|ctx| ctx.eval::<(), _>(source).map_err(|error| error.to_string()));
    if let Err(problem) = started {
        clock.pause();
        return (
            Err(cut_off(stop, &clock, budget_ms, problem)),
            String::new(),
        );
    }

    let mut calls = 0usize;
    loop {
        if let Err(problem) = pump(&runtime) {
            clock.pause();
            return (
                Err(cut_off(stop, &clock, budget_ms, problem)),
                console(&bridge),
            );
        }
        if let Some(outcome) = slot(&bridge.result).take() {
            clock.pause();
            return (outcome, console(&bridge));
        }
        let Some((id, name, arguments)) = slot(&bridge.calls).pop_front() else {
            clock.pause();
            let problem = if stop.load(Ordering::SeqCst) {
                "The script was stopped.".to_owned()
            } else {
                "The script awaited something that never happens.".to_owned()
            };
            return (Err(problem), console(&bridge));
        };
        calls += 1;
        if calls > MAX_CALLS {
            clock.pause();
            let problem = format!("The script made more than {MAX_CALLS} tool calls.");
            return (Err(problem), console(&bridge));
        }
        if events
            .send(ScriptEvent::Call(ScriptCall {
                id,
                name,
                arguments,
            }))
            .is_err()
        {
            return (Err("The script was stopped.".to_owned()), console(&bridge));
        }
        clock.pause();
        let reply = replies.recv();
        clock.resume();
        let Ok(reply) = reply else {
            return (Err("The script was stopped.".to_owned()), console(&bridge));
        };
        if let Err(problem) = settle(&context, &reply) {
            return (Err(problem), console(&bridge));
        }
        if stop.load(Ordering::SeqCst) {
            return (Err("The script was stopped.".to_owned()), console(&bridge));
        }
    }
}

/// Register the three host functions and the prelude.
fn install(context: &Context, bridge: &Arc<Bridge>) -> Result<(), String> {
    let calls = Arc::clone(bridge);
    let logged = Arc::clone(bridge);
    let settled = Arc::clone(bridge);
    context.with(|ctx| -> Result<(), String> {
        let globals = ctx.globals();
        let call = Function::new(
            ctx.clone(),
            move |id: u64, name: String, arguments: String| {
                calls
                    .calls
                    .lock()
                    .expect("the call queue is not poisoned")
                    .push_back((id, name, arguments));
            },
        )
        .map_err(|error| format!("The sandbox did not start: {error}"))?;
        let console = Function::new(ctx.clone(), move |level: String, text: String| {
            let mut kept = logged
                .console
                .lock()
                .expect("the console buffer is not poisoned");
            if kept.chars().count() >= MAX_CONSOLE_CHARS {
                return;
            }
            if !kept.is_empty() {
                kept.push('\n');
            }
            kept.push_str(&level);
            kept.push_str(": ");
            kept.push_str(&text);
        })
        .map_err(|error| format!("The sandbox did not start: {error}"))?;
        let result = Function::new(ctx.clone(), move |ok: bool, payload: String| {
            let mut slot = settled
                .result
                .lock()
                .expect("the result slot is not poisoned");
            *slot = Some(if ok { Ok(payload) } else { Err(payload) });
        })
        .map_err(|error| format!("The sandbox did not start: {error}"))?;
        globals
            .set("__wassh_call", call)
            .and_then(|()| globals.set("__wassh_console", console))
            .and_then(|()| globals.set("__wassh_result", result))
            .map_err(|error| format!("The sandbox did not start: {error}"))?;
        ctx.eval::<(), _>(PRELUDE)
            .map_err(|error| format!("The sandbox did not start: {error}"))
    })
}

/// Resolve or reject the promise the script is suspended on.
fn settle(context: &Context, reply: &ScriptReply) -> Result<(), String> {
    context.with(|ctx| {
        let settle: Function = ctx
            .globals()
            .get("__wassh_settle")
            .map_err(|error| format!("The sandbox settled a call badly: {error}"))?;
        settle
            .call::<_, ()>((reply.id, reply.ok, reply.text.clone()))
            .map_err(|error| format!("The sandbox settled a call badly: {error}"))
    })
}

/// Run the microtasks a script queued; an exception inside one is the script's failure.
fn pump(runtime: &Runtime) -> Result<(), String> {
    loop {
        match runtime.execute_pending_job() {
            Ok(true) => continue,
            Ok(false) => return Ok(()),
            Err(error) => return Err(format!("The script failed: {error}")),
        }
    }
}

/// A stop or a spent budget reads better than the engine's interrupt message.
fn cut_off(stop: &AtomicBool, clock: &Clock, budget_ms: u64, problem: String) -> String {
    if stop.load(Ordering::SeqCst) {
        "The script was stopped.".to_owned()
    } else if clock.used_ms() > budget_ms {
        format!("The script ran longer than {budget_ms} ms of execution.")
    } else {
        problem
    }
}

/// Cut a result to what the model reads.
pub fn clamp_result(text: String) -> String {
    if text.chars().count() <= MAX_RESULT_CHARS {
        return text;
    }
    let kept: String = text.chars().take(MAX_RESULT_CHARS).collect();
    format!("{kept}\n…[result truncated at {MAX_RESULT_CHARS} characters]")
}

fn slot<T>(lock: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    lock.lock().expect("the sandbox state is not poisoned")
}

fn console(bridge: &Bridge) -> String {
    slot(&bridge.console).clone()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// Milliseconds a fresh clock may report on top of the time since it was created.
    const CLOCK_SLACK_MS: u64 = 50;

    fn drive(
        job: ScriptJob,
        answers: impl Fn(&ScriptCall) -> ScriptReply,
    ) -> (Result<String, String>, String) {
        loop {
            let event = job
                .events
                .recv_timeout(Duration::from_millis(TEST_WAIT_MS))
                .expect("the sandbox went quiet");
            match event {
                ScriptEvent::Call(call) => job.reply(answers(&call)),
                ScriptEvent::Finished { result, console } => return (result, console),
            }
        }
    }

    fn ok(call: &ScriptCall, text: &str) -> ScriptReply {
        ScriptReply {
            id: call.id,
            ok: true,
            text: text.to_owned(),
        }
    }

    #[test]
    fn a_script_calls_tools_and_returns_its_own_compact_result() {
        let job = spawn(
            "const a = await tools.remote_fs_list_dir({ path: '/' });\n\
             const b = await tools.dev_grep_search({ pattern: 'x' });\n\
             return { files: a.text.length, hits: b.text };",
        );
        let (result, _) = drive(job, |call| match call.name.as_str() {
            "remote_fs_list_dir" => ok(call, r#"{"ok":true,"text":"abc"}"#),
            _ => ok(call, r#"{"ok":true,"text":"3 hits"}"#),
        });
        assert_eq!(result.unwrap(), r#"{"files":3,"hits":"3 hits"}"#);
    }

    #[test]
    fn a_failed_tool_rejects_the_call_and_the_script_can_catch_it() {
        let job = spawn(
            "try { await tools.run_command({ command: 'rm -rf /' }); return 'ran'; }\n\
             catch (error) { return { caught: true, message: error.message }; }",
        );
        let (result, _) = drive(job, |call| ScriptReply {
            id: call.id,
            ok: false,
            text: "not allowed".to_owned(),
        });
        assert_eq!(
            result.unwrap(),
            r#"{"caught":true,"message":"not allowed"}"#
        );
    }

    #[test]
    fn a_script_that_throws_reports_the_error() {
        let job = spawn("throw new Error('boom');");
        let (result, _) = drive(job, |call| ok(call, "{}"));
        let problem = result.expect_err("the script failed");
        assert!(problem.contains("boom"), "{problem}");
    }

    #[test]
    fn a_script_without_a_return_value_reports_null_and_keeps_its_console_output() {
        let job = spawn("console.log('scanning', 3); const unused = 1;");
        let (result, console) = drive(job, |call| ok(call, "{}"));
        assert_eq!(result.unwrap(), "null");
        assert_eq!(console, "log: scanning 3");
    }

    #[test]
    fn a_new_clock_does_not_count_the_time_before_it_existed() {
        // The interrupt handler reads the clock while the prelude is evaluated, before the
        // first pause. A clock that starts at zero reports the process uptime instead, so
        // every run was interrupted once the app had been up longer than the budget.
        let before = monotonic_ms();
        let clock = Clock::new();
        let used = clock.used_ms();
        assert!(
            used <= monotonic_ms().saturating_sub(before) + CLOCK_SLACK_MS,
            "a fresh clock reported {used} ms"
        );
    }

    #[test]
    fn a_runaway_script_is_cut_off() {
        let job = spawn_with_budget("while (true) {}", TEST_EXEC_MS);
        let (result, _) = drive(job, |call| ok(call, "{}"));
        let problem = result.expect_err("the script was stopped");
        assert!(problem.contains("ran longer"), "{problem}");
    }

    #[test]
    fn a_script_that_waits_forever_is_reported_instead_of_hanging() {
        let job = spawn("await new Promise(function () {});");
        let (result, _) = drive(job, |call| ok(call, "{}"));
        assert!(result.is_err());
    }
}
