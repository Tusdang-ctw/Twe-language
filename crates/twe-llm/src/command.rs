//! web3d-M5: any program as a model. The request, flattened to text
//! ([`Request::flatten`]), goes to the program's stdin; its stdout is
//! the reply. Works with a CLI (`claude -p`), a wrapper script, or a
//! local `llama-cli` with Twe's GBNF grammar, without rebuilding twec.
//! No token accounting: the program doesn't report any.

use std::io::Write;
use std::process::{Command, Stdio};

use crate::{Error, Provider, Reply, Request};

pub struct CommandProvider {
    pub program: String,
    pub args: Vec<String>,
}

impl CommandProvider {
    pub fn new(program: impl Into<String>, args: impl IntoIterator<Item = String>) -> Self {
        CommandProvider {
            program: program.into(),
            args: args.into_iter().collect(),
        }
    }
}

impl Provider for CommandProvider {
    fn complete(&mut self, request: &Request) -> Result<Reply, Error> {
        let mut child = Command::new(&self.program)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Error::Transport(format!("starting `{}` failed: {e}", self.program)))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(request.flatten().as_bytes()).map_err(|e| {
                Error::Transport(format!("writing the prompt to `{}` failed: {e}", self.program))
            })?;
        }
        let output = child
            .wait_with_output()
            .map_err(|e| Error::Transport(format!("waiting on `{}` failed: {e}", self.program)))?;
        if !output.status.success() {
            return Err(Error::Transport(format!(
                "`{}` exited with {}: {}",
                self.program,
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let text = String::from_utf8(output.stdout)
            .map_err(|e| Error::Malformed(format!("`{}` wrote non-UTF-8 output: {e}", self.program)))?;
        Ok(Reply::text_only(text, self.id()))
    }

    fn id(&self) -> String {
        format!("command:{}", self.program)
    }
}
