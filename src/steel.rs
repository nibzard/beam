// ABOUTME: Steel computers as beam sandboxes, through the `steel` CLI (preview 0.5 or later).
// ABOUTME: exec has exit codes but no stdin; ssh has stdin but no exit codes and no tty for commands.

use crate::util::{now_unix, sh_quote};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub const BOOTSTRAP_SH: &str = include_str!("../scripts/steel_bootstrap.sh");

/// The longest command that "steel computer exec" allows.
const EXEC_TIMEOUT: &str = "3600";

/// The steel CLI: from PATH, or from the place where its installer puts it.
pub fn steel() -> Command {
    let in_path = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join("steel").is_file()))
        .unwrap_or(false);
    let bin = if in_path {
        PathBuf::from("steel")
    } else {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".steel/bin/steel"))
            .unwrap_or_else(|| PathBuf::from("steel"))
    };
    let mut c = Command::new(bin);
    c.stdin(Stdio::null());
    c
}

/// Run a steel command with --json and return the "data" object.
/// Checks the CLI version first, because an old CLI fails later with an unclear error.
pub fn preflight() -> Result<()> {
    let version = crate::util::run(steel().arg("--version")).context(
        "Steel requires the Steel CLI preview. See the Steel install command in README.md",
    )?;
    if let Some(e) = version_error(&version) {
        bail!("{e}. See the Steel install command in README.md");
    }
    crate::util::run(steel().args(["computer", "quota", "--json"]))
        .context("Steel requires valid credentials. Set STEEL_API_KEY")?;
    Ok(())
}

/// Returns an error for a CLI older than 0.5, which has no `computer` commands.
/// Output that does not parse gives no error; the quota check then finds the problem.
fn version_error(output: &str) -> Option<String> {
    let version = output.split_whitespace().nth(1)?;
    let mut parts = version.split(['.', '-']);
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next()?.parse().ok()?;
    ((major, minor) < (0, 5))
        .then(|| format!("Steel CLI is too old: found {version}, need 0.5.0-preview.6 or later"))
}

fn json(cmd: &mut Command) -> Result<Value> {
    let output = cmd
        .arg("--json")
        .output()
        .context("cannot start the Steel command")?;
    let out = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() {
        bail!(
            "Steel command failed ({}): {} {}",
            output.status,
            out.trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let v: Value =
        serde_json::from_str(&out).with_context(|| format!("steel printed no JSON: {out}"))?;
    if v["success"] != Value::Bool(true) {
        bail!("steel failed: {out}");
    }
    Ok(v["data"].clone())
}

/// Create a computer (or restore a checkpoint) and wait until it runs. Returns its id.
pub fn create(
    checkpoint: Option<&str>,
    timeout_secs: u64,
    receipt: &std::path::Path,
) -> Result<String> {
    if receipt.exists() {
        let bytes = std::fs::read(receipt)?;
        let v: Value = serde_json::from_slice(&bytes).context("Steel allocation was interrupted. Find the computer ID with `steel computer list --json`, then run `beam --recover-sandbox ID`. Do not create another computer")?;
        return v["data"]["id"].as_str().map(str::to_string).context("Steel allocation has no computer ID. Inspect `steel computer list --json`, then use `beam --recover-sandbox ID`");
    }
    let mut c = steel();
    match checkpoint {
        Some(id) => c.args(["checkpoint", "restore", id]),
        None => c.args(["computer", "create"]),
    };
    c.args(["--auto-pause", "--timeout", &timeout_secs.to_string()]);
    let file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(receipt)?;
    let out = c
        .arg("--json")
        .stdout(file.try_clone()?)
        .stderr(Stdio::piped())
        .output()?;
    file.sync_all()?;
    if !out.status.success() {
        bail!(
            "Steel allocation failed: {}. Inspect `steel computer list --json` before retrying",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let v: Value = serde_json::from_slice(&std::fs::read(receipt)?)?;
    let id = v["data"]["id"]
        .as_str()
        .context("Steel returned no computer ID")?;
    Ok(id.to_string())
}

/// Run a script with "steel computer exec". Stdout and stderr come back together.
pub fn exec(id: &str, script: &str) -> Result<String> {
    let data = json(steel().args([
        "computer",
        "exec",
        id,
        "--timeout",
        EXEC_TIMEOUT,
        "-c",
        script,
    ]))?;
    let output = data["output"].as_str().unwrap_or_default().to_string();
    if data["timedOut"] == Value::Bool(true) {
        bail!("command timed out in the Steel computer: {output}");
    }
    if data["truncated"] == Value::Bool(true) {
        bail!("the output of a command in the Steel computer was cut off");
    }
    let code = data["exitCode"].as_i64().unwrap_or(-1);
    if code != 0 {
        bail!(
            "command failed in the Steel computer (exit {code}): {}",
            output.trim()
        );
    }
    Ok(output.trim().to_string())
}

/// Run a script with "steel computer ssh", with stdin and stdout connected to `cmd`'s pipes.
/// ssh always exits 0, so the script writes its exit code to a file that exec then reads.
pub fn ssh_output(id: &str, script: &str, stdin: Stdio) -> Result<Vec<u8>> {
    let rc = format!("/tmp/beam-rc-{}-{}", std::process::id(), now_unix());
    let wrapper = format!("sh -c {} ; echo $? > {}", sh_quote(script), sh_quote(&rc));
    let out = steel()
        .args(["computer", "ssh", id, "--", "sh", "-c", &wrapper])
        .stdin(stdin)
        .output()
        .context("cannot start steel computer ssh")?;
    let code = exec(
        id,
        &format!(
            "cat {rc} 2>/dev/null || echo missing; rm -f {rc}",
            rc = sh_quote(&rc)
        ),
    )?;
    let code = code.trim();
    if code == "141" {
        return Err(ConnectionClosed(ssh_failure(code, &out.stdout, &out.stderr)).into());
    }
    if code != "0" {
        bail!(ssh_failure(code, &out.stdout, &out.stderr));
    }
    Ok(out.stdout)
}

/// The error when the ssh connection closes while its command runs. Then the command got
/// SIGPIPE (exit 141) when it wrote to the closed output. It is safe to run the command again.
#[derive(Debug)]
pub struct ConnectionClosed(String);

impl std::fmt::Display for ConnectionClosed {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConnectionClosed {}

/// The most output text that a failure message keeps. The end of the output has the error.
const FAILURE_OUTPUT: usize = 2000;

/// The message for a failed ssh command: the exit code, the cause if beam knows it, and the
/// end of the output. ssh without a tty can send the remote stderr on stdout, so keep both.
fn ssh_failure(code: &str, stdout: &[u8], stderr: &[u8]) -> String {
    let mut msg = format!("command failed in the Steel computer (exit {code})");
    if code == "141" {
        msg.push_str(". The connection to the Steel computer closed while the command ran");
    }
    let output = format!(
        "{}\n{}",
        String::from_utf8_lossy(stdout).trim(),
        String::from_utf8_lossy(stderr).trim()
    );
    let output = output.trim();
    let skip = output.chars().count().saturating_sub(FAILURE_OUTPUT);
    let tail: String = output.chars().skip(skip).collect();
    if !tail.is_empty() {
        msg.push_str(": ");
        if skip > 0 {
            msg.push('…');
        }
        msg.push_str(&tail);
    }
    msg
}

/// The marker that the attach command writes when `script` ends.
const ATTACH_DONE: &str = "$HOME/.beam-attach-done";

/// The command for ~/.beam-attach: run `script`, then write the done marker.
fn attach_command(script: &str) -> String {
    format!("{script}; touch \"{ATTACH_DONE}\"")
}

/// Connect this terminal to `script`. Only a login shell has a terminal, so the script goes
/// into ~/.beam-attach and the hook in ~/.bashrc (see steel_bootstrap.sh) runs it.
/// "steel computer ssh" does not stop when the remote shell exits. Thus beam polls for the
/// done marker, stops the client with SIGTERM, and restores the terminal mode.
pub fn interactive(id: &str, script: &str) -> Result<()> {
    exec(
        id,
        &format!(
            "rm -f \"{ATTACH_DONE}\"; printf '%s' {} > \"$HOME/.beam-attach\"",
            sh_quote(&attach_command(script))
        ),
    )?;
    let saved = Command::new("stty")
        .arg("-g")
        .stdin(Stdio::inherit())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    let mut child = steel()
        .args(["computer", "ssh", id])
        .stdin(Stdio::inherit())
        .spawn()
        .context("cannot start steel computer ssh")?;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        std::thread::sleep(std::time::Duration::from_secs(3));
        if exec(
            id,
            &format!("test -f \"{ATTACH_DONE}\" && echo done || true"),
        )
        .is_ok_and(|out| out == "done")
        {
            let _ = Command::new("kill")
                .args(["-TERM", &child.id().to_string()])
                .status();
            let _ = child.wait();
            break None;
        }
    };
    if let Some(saved) = saved {
        let _ = Command::new("stty")
            .arg(saved)
            .stdin(Stdio::inherit())
            .status();
    }
    if let Some(status) = status
        && !status.success()
    {
        bail!("steel computer ssh stopped with {status}");
    }
    Ok(())
}

/// Resume the computer when it paused itself (after --auto-pause).
pub fn status(id: &str) -> Result<String> {
    let data = json(steel().args(["computer", "get", id]))?;
    Ok(data["status"].as_str().unwrap_or("unknown").to_string())
}

pub fn wake(id: &str) -> Result<()> {
    let data = json(steel().args(["computer", "get", id]))?;
    match data["status"].as_str().unwrap_or_default() {
        "running" => Ok(()),
        "paused" => {
            crate::up::step("sandbox", "resuming the paused Steel computer");
            json(steel().args(["computer", "resume", id, "--wait"])).map(|_| ())
        }
        s => bail!("the Steel computer {id} is {s:?}. It must be running or paused"),
    }
}

pub fn delete(id: &str) -> Result<()> {
    match json(steel().args(["computer", "delete", id])) {
        Ok(_) => Ok(()),
        Err(e)
            if e.to_string().to_lowercase().contains("not found")
                || e.to_string().contains("404") =>
        {
            Ok(())
        }
        Err(e) => Err(e),
    }
}

pub fn download(id: &str, script: &str, file: &std::fs::File) -> Result<()> {
    let rc = format!("/tmp/beam-download-{}-{}", std::process::id(), now_unix());
    let wrapper = format!("sh -c {} ; echo $? > {}", sh_quote(script), sh_quote(&rc));
    let out = steel()
        .args(["computer", "ssh", id, "--", "sh", "-c", &wrapper])
        .stdin(Stdio::null())
        .stdout(file.try_clone()?)
        .stderr(Stdio::piped())
        .output()?;
    let code = exec(
        id,
        &format!(
            "cat {0} 2>/dev/null || echo missing; rm -f {0}",
            sh_quote(&rc)
        ),
    )?;
    if !out.status.success() || code.trim() != "0" {
        bail!(
            "Steel download failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

pub fn ready(id: &str) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
    loop {
        let data = json(steel().args(["computer", "get", id]))?;
        match data["status"].as_str().unwrap_or_default() {
            "running" => return Ok(()),
            "paused" => {
                wake(id)?;
                return Ok(());
            }
            "creating" | "starting" | "pending" | "provisioning" => {}
            status => {
                bail!("Steel computer {id} is {status}. Inspect it with `steel computer get {id}`")
            }
        }
        if std::time::Instant::now() > deadline {
            bail!("Steel computer is still starting. Run `beam` to retry");
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_error_names_an_old_cli() {
        let e = version_error("steel 0.4.4").unwrap();
        assert!(e.contains("found 0.4.4"), "{e}");
        assert!(e.contains("need 0.5.0-preview.6 or later"), "{e}");
    }

    #[test]
    fn version_error_accepts_the_preview_and_later() {
        assert_eq!(version_error("steel 0.5.0-preview.6"), None);
        assert_eq!(version_error("steel 0.5.1"), None);
        assert_eq!(version_error("steel 1.0.0"), None);
    }

    #[test]
    fn version_error_ignores_unknown_output() {
        assert_eq!(version_error(""), None);
        assert_eq!(version_error("steel dev"), None);
    }

    #[test]
    fn attach_command_marks_the_end_after_the_script() {
        let d = tempfile::tempdir().unwrap();
        let out = Command::new("sh")
            .args(["-c", &attach_command("printf ran > \"$HOME/order\"")])
            .env("HOME", d.path())
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(
            std::fs::read_to_string(d.path().join("order")).unwrap(),
            "ran"
        );
        assert!(d.path().join(".beam-attach-done").exists());
    }

    #[test]
    fn failed_command_preserves_json_stdout_and_stderr() {
        let error = json(Command::new("sh").args([
            "-c",
            r#"printf '%s' '{"success":true,"data":{"exitCode":4,"output":"missing tools: cargo"}}'; echo 'diagnostic on stderr' >&2; exit 4"#,
        ]))
        .unwrap_err()
        .to_string();
        assert!(error.contains("missing tools: cargo"), "{error}");
        assert!(error.contains("diagnostic on stderr"), "{error}");
        assert!(error.contains("exit status: 4"), "{error}");
    }

    #[test]
    fn ssh_failure_explains_a_closed_connection_and_keeps_output() {
        let msg = ssh_failure("141", b"tar: Unexpected EOF in archive\n", b"");
        assert!(msg.contains("exit 141"), "{msg}");
        assert!(
            msg.contains("connection to the Steel computer closed"),
            "{msg}"
        );
        assert!(msg.contains("tar: Unexpected EOF in archive"), "{msg}");
    }

    #[test]
    fn ssh_failure_keeps_only_the_end_of_long_output() {
        let mut out = "x".repeat(10_000).into_bytes();
        out.extend_from_slice(b"last line");
        let msg = ssh_failure("2", &out, b"stderr text");
        assert!(
            msg.starts_with("command failed in the Steel computer (exit 2)"),
            "{msg}"
        );
        assert!(!msg.contains("connection"), "{msg}");
        assert!(
            msg.contains("last line") && msg.contains("stderr text"),
            "{msg}"
        );
        assert!(msg.len() < 3_000, "{}", msg.len());
    }
}
