//! A tiny, self-contained [`CodeRuntime`] for the CodeAct example.
//!
//! Production CodeAct uses a real interpreter (the intended adapter wraps
//! Pydantic's Monty, a Rust-native Python). To keep this example runnable with
//! no native dependencies, `LineScriptRuntime` interprets a deliberately minimal
//! *line script* language while still exercising the full [`CodeRuntime`] seam,
//! including suspend/resume at a call boundary.
//!
//! # Language
//!
//! One instruction per line; blank lines and `#` comments are ignored:
//!
//! - `CALL <tool> <json-args>` — call a tool; its result becomes `$last`.
//! - `OBSERVE <json>` — return an observation to the model (continues the loop).
//! - `FINAL <json|$last>` — return the final result and end the loop.
//!
//! The "continuation" is just the remaining lines plus the last tool result, so
//! it serializes trivially — which is exactly what makes suspend/resume work.

use adk_agent::codeact::{
    CodeRuntime, PendingCall, ResumeWith, RunStep, RuntimeCapabilities, RuntimeError,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The serializable interpreter state: the lines still to run and the most
/// recent tool result (bound to `$last`).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Program {
    lines: Vec<String>,
    last: Value,
    next_call_id: u64,
}

impl Program {
    fn parse(script: &str) -> Self {
        let lines = script
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(str::to_string)
            .collect();
        Self { lines, last: Value::Null, next_call_id: 1 }
    }
}

/// Advance the program by one instruction to its next host-relevant stop.
///
/// Each instruction maps to exactly one [`RunStep`]: a `CALL` yields a pending
/// call (with the remaining program as its continuation), while `OBSERVE`/`FINAL`
/// complete the script. Blank and comment lines were already stripped in
/// [`Program::parse`], so there is nothing to skip here.
fn step(mut program: Program) -> Result<RunStep, RuntimeError> {
    if program.lines.is_empty() {
        // No FINAL was reached: report it as a script error the model can react to.
        return Ok(RunStep::Raised("script ended without a FINAL result".to_string()));
    }
    let line = program.lines.remove(0);
    let (op, rest) = match line.split_once(char::is_whitespace) {
        Some((op, rest)) => (op, rest.trim()),
        None => (line.as_str(), ""),
    };
    match op {
        "CALL" => {
            let (name, args_str) = rest
                .split_once(char::is_whitespace)
                .map(|(n, a)| (n.trim(), a.trim()))
                .unwrap_or((rest, "{}"));
            let args: Value = serde_json::from_str(args_str)
                .map_err(|e| RuntimeError::Parse(format!("bad CALL args: {e}")))?;
            let call_id = program.next_call_id;
            program.next_call_id += 1;
            Ok(RunStep::Call(Box::new(LinePendingCall {
                name: name.to_string(),
                args,
                call_id,
                remaining: program,
            })))
        }
        "OBSERVE" => {
            let value = resolve(rest, &program.last)?;
            Ok(RunStep::Complete(json!({"type": "observation", "value": value})))
        }
        "FINAL" => {
            let value = resolve(rest, &program.last)?;
            Ok(RunStep::Complete(json!({"type": "final_result", "value": value})))
        }
        other => Ok(RunStep::Raised(format!("unknown instruction: {other}"))),
    }
}

/// Resolve a literal JSON argument, or the special `$last` token, to a value.
fn resolve(text: &str, last: &Value) -> Result<Value, RuntimeError> {
    if text == "$last" {
        return Ok(last.clone());
    }
    serde_json::from_str(text).map_err(|e| RuntimeError::Parse(format!("bad JSON value: {e}")))
}

/// A self-contained [`CodeRuntime`] over the line-script language.
pub struct LineScriptRuntime;

impl CodeRuntime for LineScriptRuntime {
    fn start(&self, script: &str, _script_name: &str) -> Result<RunStep, RuntimeError> {
        step(Program::parse(script))
    }

    fn resume(&self, snapshot: &[u8], with: ResumeWith) -> Result<RunStep, RuntimeError> {
        let mut program: Program =
            serde_json::from_slice(snapshot).map_err(|e| RuntimeError::Snapshot(e.to_string()))?;
        match with {
            ResumeWith::Value(value) => {
                program.last = value;
                step(program)
            }
            // A raised error in this toy language simply ends the script; a real
            // runtime would inject it at the call site so the script could catch it.
            ResumeWith::Raise(message) => Ok(RunStep::Raised(message)),
        }
    }

    fn capabilities(&self) -> RuntimeCapabilities {
        RuntimeCapabilities::new(
            true,
            "You are writing a minimal LINE SCRIPT. One instruction per line:\n\
             - CALL <tool> <json-args>   call a tool; its result is bound to $last\n\
             - OBSERVE <json|$last>      surface info to yourself and continue\n\
             - FINAL <json|$last>        return the final result and stop\n\
             Emit exactly one fenced code block containing the script.",
        )
    }
}

/// A paused tool call: its continuation is just the remaining [`Program`].
struct LinePendingCall {
    name: String,
    args: Value,
    call_id: u64,
    remaining: Program,
}

impl PendingCall for LinePendingCall {
    fn function_name(&self) -> &str {
        &self.name
    }

    fn args(&self) -> &Value {
        &self.args
    }

    fn call_id(&self) -> u64 {
        self.call_id
    }

    fn dump(&self) -> Result<Vec<u8>, RuntimeError> {
        serde_json::to_vec(&self.remaining).map_err(|e| RuntimeError::Snapshot(e.to_string()))
    }

    fn resume(self: Box<Self>, with: ResumeWith) -> Result<RunStep, RuntimeError> {
        let mut remaining = self.remaining;
        match with {
            ResumeWith::Value(value) => {
                remaining.last = value;
                step(remaining)
            }
            ResumeWith::Raise(message) => Ok(RunStep::Raised(message)),
        }
    }
}
