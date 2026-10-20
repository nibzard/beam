// ABOUTME: Shell scripts that beam runs in the sandbox, with their variables filled in.
// ABOUTME: Each function returns a full script as a string. The provider runs it with "sh -c".

use crate::util::sh_quote;

pub const RESTORE_SH: &str = include_str!("../scripts/restore.sh");
pub const RUN_SH: &str = include_str!("../scripts/run.sh");
pub const PACK_BACK_SH: &str = include_str!("../scripts/pack_back.sh");
pub const REMOTE_BEAM_SH: &str = include_str!("../scripts/remote_beam.sh");

/// Put `NAME='value'` lines before a script body.
pub fn with_vars(vars: &[(&str, &str)], body: &str) -> String {
    let mut s = String::new();
    for (k, v) in vars {
        s.push_str(&format!("{k}={}\n", sh_quote(v)));
    }
    s.push_str(body);
    s
}

/// Claim only newly created paths. Existing paths must have this transfer's owner marker.
pub fn prepare(home: &str, project: &str, stage: &str, owner: &str) -> String {
    with_vars(
        &[("H", home), ("P", project), ("S", stage), ("OWNER", owner)],
        r#"set -eu
umask 077
claim() {
  path=$1 marker=$2
  if [ -e "$path" ]; then
    actual=$(cat "$path/$marker" 2>/dev/null || true)
    if [ "$marker" = .beam-owner ] && [ -f "$path/.git/beam-owner" ]; then actual=$(cat "$path/.git/beam-owner"); fi
    [ ! -L "$path" ] && [ "$actual" = "$OWNER" ] || {
      echo "beam: $path already exists and is not owned by this transfer" >&2; exit 3;
    }
  else
    parent=$(dirname "$path")
    mkdir -p "$parent" 2>/dev/null || sudo -n mkdir -p "$parent"
    if ! mkdir "$path" 2>/dev/null; then
      sudo -n mkdir "$path" && sudo -n chown "$(id -u):$(id -g)" "$path"
    fi
    printf '%s' "$OWNER" > "$path/$marker"
  fi
}
claim "$S" owner
claim "$P" .beam-owner
mkdir -p "$H"
"#,
    )
}

pub fn unpack(stage: &str) -> String {
    with_vars(
        &[("S", stage)],
        "set -eu\nmkdir -p \"$S\"\ntar xzf - -C \"$S\" --no-same-owner\n",
    )
}

pub fn write_env(stage: &str) -> String {
    with_vars(&[("S", stage)], "set -eu\numask 077\ncat > \"$S/env\"\n")
}

pub struct RestoreVars<'a> {
    pub stage: &'a str,
    pub project: &'a str,
    pub home: &'a str,
    pub refname: &'a str,
    pub branch: &'a str,
    pub head: &'a str,
    pub idx_tree: &'a str,
    pub wt_tree: &'a str,
    pub origin: &'a str,
    pub git_name: &'a str,
    pub git_email: &'a str,
}

pub fn restore(v: &RestoreVars) -> String {
    with_vars(
        &[
            ("S", v.stage),
            ("P", v.project),
            ("H", v.home),
            ("REF", v.refname),
            ("BRANCH", v.branch),
            ("HEAD_SHA", v.head),
            ("IDX_TREE", v.idx_tree),
            ("WT_TREE", v.wt_tree),
            ("ORIGIN", v.origin),
            ("GIT_NAME", v.git_name),
            ("GIT_EMAIL", v.git_email),
        ],
        RESTORE_SH,
    )
}

pub struct RunVars<'a> {
    pub stage: &'a str,
    pub cwd: &'a str,
    pub home: &'a str,
    pub handoff_head: &'a str,
    pub setup: &'a [String],
    pub verify: &'a [String],
    pub reuse_setup: bool,
    pub setup_inputs: &'a [String],
    pub tools: &'a [String],
    pub versions: &'a [crate::config::ToolVersion],
    pub environment_repair: bool,
    pub resume_fn: &'a str,
    pub install: &'a ToolInstall,
}

/// Missing known tools that the launcher installs before the prerequisite check, and their versions.
/// An empty version means that the project does not need the tool.
#[derive(Debug, Default, Clone)]
pub struct ToolInstall {
    pub enabled: bool,
    pub node: String,
    pub pnpm: String,
    pub uv: String,
    pub rust: String,
}

impl ToolInstall {
    /// The plan line, for example "installs Node.js 22 and pnpm 11.11.0 if missing".
    pub fn describe(&self) -> Option<String> {
        if !self.enabled {
            return None;
        }
        let mut names = vec![];
        if !self.node.is_empty() {
            names.push(format!("Node.js {}", self.node));
        }
        if !self.pnpm.is_empty() {
            names.push(format!("pnpm {}", self.pnpm));
        }
        if !self.uv.is_empty() {
            names.push(format!("uv {}", self.uv));
        }
        if !self.rust.is_empty() {
            names.push(format!("Rust {}", self.rust));
        }
        (!names.is_empty()).then(|| {
            format!(
                "installs {} if missing; existing versions stay",
                names.join(" and ")
            )
        })
    }
}

/// The launcher that tmux runs. It keeps the PATH entries of the sandbox user, then uses $H as HOME.
pub fn run_script(v: &RunVars) -> String {
    let setup = v.setup.join("\n");
    let mut s = with_vars(
        &[
            ("S", v.stage),
            ("P", v.cwd),
            ("H", v.home),
            ("HANDOFF_HEAD", v.handoff_head),
            ("HANDOFF_TAIL", crate::handoff::TAIL),
            ("SETUP", &setup),
            ("VERIFY", &v.verify.join("\n")),
            ("REUSE_SETUP", if v.reuse_setup { "yes" } else { "no" }),
            ("SETUP_INPUTS", &v.setup_inputs.join("\n")),
            ("TOOLS", &v.tools.join("\n")),
        ],
        "",
    );
    s.push_str("export PATH=\"$PATH:$HOME/.cargo/bin:$HOME/.local/bin:$HOME/.npm-global/bin:/usr/local/bin\"\n");
    s.push_str(v.resume_fn);
    s.push('\n');
    finish_run_script(
        &s,
        &crate::sandbox::prerequisite_script(v.tools, v.versions),
        v.environment_repair,
        v.install,
    )
}

const REPAIR_LAUNCHER: &str = "# beam-launcher: environment-repair-v1\n";

fn finish_run_script(
    prefix: &str,
    prerequisites: &str,
    repair: bool,
    install: &ToolInstall,
) -> String {
    let prefix = format!(
        "{prefix}\n{}",
        with_vars(
            &[
                ("PREREQUISITES", prerequisites),
                ("ENVIRONMENT_REPAIR", if repair { "yes" } else { "no" }),
                ("INSTALL_TOOLS", if install.enabled { "yes" } else { "no" }),
                ("NODE_VERSION", &install.node),
                ("PNPM_VERSION", &install.pnpm),
                ("UV_VERSION", &install.uv),
                ("RUST_VERSION", &install.rust),
            ],
            ""
        )
    );
    let checks = format!(
        "{prefix}{}{}",
        include_str!("../scripts/tools.sh"),
        include_str!("../scripts/check.sh")
    );
    format!(
        "{REPAIR_LAUNCHER}{prefix}{}",
        with_vars(&[("CHECK_SCRIPT", &checks)], RUN_SH)
    )
}

/// Upgrade a saved, not-yet-started launcher without rebuilding the workspace snapshot.
pub fn upgrade_run_script(
    script: &str,
    prerequisites: &str,
    repair: bool,
) -> anyhow::Result<Option<String>> {
    if script.starts_with(REPAIR_LAUNCHER) {
        return Ok(None);
    }
    let (prefix, _) = script.rsplit_once("\n# ABOUTME: Runs setup and project checks,")
        .ok_or_else(|| anyhow::anyhow!("saved launcher format is unsupported; keep the saved transfer and inspect its run.sh"))?;
    // A saved launcher keeps its original behavior: Beam does not add tool installation.
    Ok(Some(finish_run_script(
        prefix,
        prerequisites,
        repair,
        &ToolInstall::default(),
    )))
}

pub fn start_tmux(stage: &str, name: &str) -> String {
    let command = format!(
        "while [ ! -f {} ]; do sleep 0.1; done; sh {}",
        sh_quote(&format!("{stage}/go")),
        sh_quote(&format!("{stage}/run.sh"))
    );
    let pipe = format!("cat >> {}", sh_quote(&format!("{stage}/terminal.log")));
    with_vars(
        &[
            ("S", stage),
            ("T", name),
            ("CMD", &command),
            ("PIPE", &pipe),
        ],
        r#"set -eu
if [ -f "$S/started" ]; then exit 0; fi
tmux has-session -t "$T" 2>/dev/null || tmux new-session -d -s "$T" -x 200 -y 50 "$CMD"
tmux pipe-pane -o -t "$T" "$PIPE"
touch "$S/go"
"#,
    )
}

pub fn retry_setup(stage: &str, name: &str) -> String {
    with_vars(
        &[("S", stage), ("T", name)],
        r#"set -eu
[ "$(cat "$S/phase" 2>/dev/null)" = needs-attention ] || exit 0
tmux kill-session -t "$T" 2>/dev/null || true
rm -f "$S/started" "$S/agent.exit" "$S/go"
rmdir "$S/run-lock" "$S/check-lock" 2>/dev/null || true
printf preparing > "$S/phase"
"#,
    )
}

/// An exited agent may have left an inspection shell; do not spend the graceful timeout on it.
pub fn stop_launched_agent(stage: &str, name: &str, graceful: bool) -> String {
    format!(
        "if [ -f {} ]; then\n{}\nelse\n{}\nfi",
        sh_quote(&format!("{stage}/agent.exit")),
        stop_agent(name, false),
        stop_agent(name, graceful)
    )
}

/// Stop a terminal session. Graceful stop sends two Ctrl-C (Claude Code exits on the second one),
/// then kills after 15 s. Otherwise, for example for an interactive shell, kill immediately.
pub fn stop_agent(name: &str, graceful: bool) -> String {
    with_vars(
        &[
            ("T", name),
            ("GRACEFUL", if graceful { "yes" } else { "no" }),
        ],
        r#"tmux has-session -t "$T" 2>/dev/null || exit 0
if [ "$GRACEFUL" != yes ]; then tmux kill-session -t "$T" 2>/dev/null; exit 0; fi
tmux send-keys -t "$T" C-c
sleep 1
# The session can end after the first Ctrl-C.
tmux send-keys -t "$T" C-c 2>/dev/null || exit 0
i=0
while tmux has-session -t "$T" 2>/dev/null; do
  i=$((i + 1))
  if [ "$i" -gt 30 ]; then tmux kill-session -t "$T"; break; fi
  sleep 0.5
done
exit 0
"#,
    )
}

/// Observe the launcher and process state. Task evidence comes from an adapter.
pub fn process_status(stage: &str, name: &str) -> String {
    with_vars(
        &[("S", stage), ("T", name)],
        include_str!("../scripts/agent_status.sh"),
    )
}

pub fn pack_back(
    stage: &str,
    project: &str,
    home: &str,
    refname: &str,
    sent: &str,
    agent_paths: &[String],
    extras: &[String],
) -> String {
    let paths = agent_paths.join("\n");
    let extras = extras.join("\n");
    with_vars(
        &[
            ("S", stage),
            ("P", project),
            ("H", home),
            ("REF", refname),
            ("SENT", sent),
            ("AGENT_PATHS", &paths),
            ("EXTRAS", &extras),
        ],
        PACK_BACK_SH,
    )
}

/// The script that `beam down` in the sandbox runs: stop the agent and the repair shell, then pack.
/// Each part runs in its own shell: each part can end with `exit`, and `set -e` has no effect
/// in a subshell on the left side of `||`.
pub fn return_script(stage: &str, tmux: &str, graceful: bool, pack: &str) -> String {
    with_vars(
        &[
            ("S", stage),
            ("STOP", &stop_launched_agent(stage, tmux, graceful)),
            ("STOP_REPAIR", &stop_agent(&format!("{tmux}-repair"), false)),
            ("PACK", pack),
        ],
        r#"set -u
event() { printf '%s\t%s\t%s\n' "$(date +%s)" "$1" "$2" >> "$S/events.tsv"; }
fail() { printf '%s\n' "$1" > "$S/return-failed"; event return-failed "$1"; echo "beam: $1" >&2; exit 1; }
sh -c "$STOP" || fail 'cannot stop the agent'
sh -c "$STOP_REPAIR" || fail 'cannot stop the repair shell'
sh -c "$PACK" || fail 'cannot pack the work'
# Record the event first: readers that see the marker also see its event.
event return-ready 'run beam down on the local machine'
date +%s > "$S/return-ready"
"#,
    )
}

/// Progress of `beam down` in the sandbox: return-ready, return-failed REASON, return-requested, or none.
pub fn return_progress(stage: &str) -> String {
    with_vars(
        &[("S", stage)],
        r#"if [ -f "$S/return-ready" ]; then echo return-ready
elif [ -f "$S/return-failed" ]; then printf 'return-failed %s\n' "$(cat "$S/return-failed")"
elif [ -f "$S/return-requested" ]; then echo return-requested
else echo none
fi
"#,
    )
}

/// Install the sandbox `beam` command in ROOT/bin. A sandbox that beam owns also gets
/// /usr/local/bin/beam, but only when that path is free or has an earlier copy of this command.
pub fn install_cli(root: &str, system: bool) -> String {
    let cli = format!(
        "#!/bin/sh\n# beam-sandbox-cli\n{}",
        with_vars(&[("ROOT", root)], REMOTE_BEAM_SH)
    );
    with_vars(
        &[
            ("ROOT", root),
            ("CLI", &cli),
            ("SYSTEM", if system { "yes" } else { "no" }),
        ],
        r#"set -eu
mkdir -p "$ROOT/bin"
printf '%s' "$CLI" > "$ROOT/bin/beam.new"
chmod 755 "$ROOT/bin/beam.new"
mv "$ROOT/bin/beam.new" "$ROOT/bin/beam"
[ "$SYSTEM" = yes ] || exit 0
dest=/usr/local/bin/beam
if [ -e "$dest" ] && ! grep -q beam-sandbox-cli "$dest" 2>/dev/null; then exit 0; fi
if ! { cp "$ROOT/bin/beam" "$dest" 2>/dev/null || sudo -n cp "$ROOT/bin/beam" "$dest" 2>/dev/null; }; then
  echo "beam: cannot install $dest; use $ROOT/bin/beam" >&2
fi
"#,
    )
}

/// The `.beam` directory that contains a stage (`ROOT/remote/ID`).
pub fn beam_root(stage: &str) -> &str {
    stage
        .rsplit_once("/remote/")
        .map(|(root, _)| root)
        .unwrap_or(stage)
}

pub fn cat(path: &str) -> String {
    format!("cat {}", sh_quote(path))
}

/// SSH cleanup checks ownership before each deletion and never deletes shared home files.
pub fn cleanup(stage: &str, project: &str, owner: &str, tmux: &str, project_owned: bool) -> String {
    with_vars(
        &[
            ("S", stage),
            ("P", project),
            ("OWNER", owner),
            ("T", tmux),
            ("EXPECTED", if project_owned { "yes" } else { "no" }),
        ],
        r#"set -eu
if [ -e "$S" ]; then
  [ ! -L "$S" ] && [ "$(cat "$S/owner" 2>/dev/null || true)" = "$OWNER" ] || {
    echo 'beam: stage ownership cannot be verified; nothing was deleted' >&2; exit 3;
  }
fi
remove_project=no
if [ -e "$P" ]; then
  actual=$(cat "$P/.git/beam-owner" 2>/dev/null || cat "$P/.beam-owner" 2>/dev/null || true)
  if [ ! -L "$P" ] && [ "$actual" = "$OWNER" ]; then
    remove_project=yes
  else
    echo 'beam: existing project is not owned by this transfer; it remains untouched' >&2
    if [ "$EXPECTED" = yes ]; then exit 3; fi
  fi
fi
# Do not touch a tmux session unless this transfer owns its stage.
if [ -f "$S/owner" ]; then
  tmux kill-session -t "$T" 2>/dev/null || true
  tmux kill-session -t "$T-repair" 2>/dev/null || true
fi
if [ "$remove_project" = yes ]; then rm -rf -- "$P"; fi
if [ -f "$S/owner" ]; then rm -rf -- "$S"; fi
"#,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vars_are_quoted() {
        let s = with_vars(&[("A", "x y"), ("B", "it's")], "echo \"$A|$B\"");
        let out = crate::util::run(std::process::Command::new("sh").arg("-c").arg(&s)).unwrap();
        assert_eq!(out, "x y|it's");
    }

    #[test]
    fn return_ready_marker_appears_only_after_its_event() {
        let d = tempfile::tempdir().unwrap();
        let stage = d.path().join("stage");
        let bin = d.path().join("bin");
        std::fs::create_dir_all(&stage).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let s = stage.to_str().unwrap();
        // Each date call checks if a reader could see the marker without its event.
        let date = format!(
            "#!/bin/sh\nif [ -e {s}/return-ready ] && ! grep -q return-ready {s}/events.tsv 2>/dev/null; then touch {s}/early; fi\necho 1\n",
            s = sh_quote(s)
        );
        std::fs::write(bin.join("date"), date).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(bin.join("date"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let script = return_script(s, "beam-no-such-session-0", false, "exit 0");
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
        let out = std::process::Command::new("sh")
            .args(["-c", &script])
            .env("PATH", path)
            .env("TMUX_TMPDIR", d.path())
            .env_remove("TMUX")
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        assert!(stage.join("return-ready").exists());
        assert!(
            !stage.join("early").exists(),
            "return-ready appeared before its event"
        );
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    use std::process::Command;
    fn shell(script: &str) -> std::process::Output {
        Command::new("sh").args(["-c", script]).output().unwrap()
    }
    #[test]
    fn existing_project_survives_prepare_and_cleanup() {
        let d = tempfile::tempdir().unwrap();
        let project = d.path().join("project");
        let home = d.path().join("home");
        let stage = d.path().join("stage");
        std::fs::create_dir(&project).unwrap();
        std::fs::write(project.join("keep"), "user work").unwrap();
        assert!(
            !shell(&prepare(
                home.to_str().unwrap(),
                project.to_str().unwrap(),
                stage.to_str().unwrap(),
                "ours"
            ))
            .status
            .success()
        );
        shell(&cleanup(
            stage.to_str().unwrap(),
            project.to_str().unwrap(),
            "ours",
            "absent-beam-test",
            false,
        ));
        assert_eq!(
            std::fs::read_to_string(project.join("keep")).unwrap(),
            "user work"
        );
    }
    #[test]
    fn owned_paths_can_be_prepared_twice_and_removed_twice() {
        let d = tempfile::tempdir().unwrap();
        let project = d.path().join("project with spaces");
        let stage = d.path().join("stage");
        let home = stage.join("home");
        let prepare = prepare(
            home.to_str().unwrap(),
            project.to_str().unwrap(),
            stage.to_str().unwrap(),
            "ours",
        );
        assert!(shell(&prepare).status.success());
        assert!(shell(&prepare).status.success());
        let cleanup = cleanup(
            stage.to_str().unwrap(),
            project.to_str().unwrap(),
            "ours",
            "absent-beam-test",
            true,
        );
        assert!(shell(&cleanup).status.success());
        assert!(shell(&cleanup).status.success());
        assert!(!project.exists());
        assert!(!stage.exists());
    }
    #[test]
    fn ownership_survives_git_clean_and_repeated_preparation() {
        let d = tempfile::tempdir().unwrap();
        let project = d.path().join("project");
        let stage = d.path().join("stage");
        let home = stage.join("home");
        let prepare = prepare(
            home.to_str().unwrap(),
            project.to_str().unwrap(),
            stage.to_str().unwrap(),
            "ours",
        );
        assert!(shell(&prepare).status.success());
        assert!(
            Command::new("git")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .arg("-C")
                .arg(&project)
                .args(["init", "-q"])
                .output()
                .unwrap()
                .status
                .success()
        );
        std::fs::rename(project.join(".beam-owner"), project.join(".git/beam-owner")).unwrap();
        std::fs::write(project.join("temporary"), "discard").unwrap();
        assert!(
            Command::new("git")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .arg("-C")
                .arg(&project)
                .args(["clean", "-fdx"])
                .output()
                .unwrap()
                .status
                .success()
        );
        assert!(shell(&prepare).status.success());
        assert!(
            shell(&cleanup(
                stage.to_str().unwrap(),
                project.to_str().unwrap(),
                "ours",
                "absent-beam-test",
                true
            ))
            .status
            .success()
        );
        assert!(!project.exists());
    }

    #[test]
    fn missing_project_ownership_keeps_the_transfer_recoverable() {
        let d = tempfile::tempdir().unwrap();
        let project = d.path().join("project");
        let stage = d.path().join("stage");
        assert!(
            shell(&prepare(
                stage.join("home").to_str().unwrap(),
                project.to_str().unwrap(),
                stage.to_str().unwrap(),
                "ours"
            ))
            .status
            .success()
        );
        std::fs::remove_file(project.join(".beam-owner")).unwrap();
        assert!(
            !shell(&cleanup(
                stage.to_str().unwrap(),
                project.to_str().unwrap(),
                "ours",
                "absent-beam-test",
                true
            ))
            .status
            .success()
        );
        assert!(stage.exists());
        assert!(project.exists());
    }

    #[test]
    fn wrong_owner_prevents_cleanup() {
        let d = tempfile::tempdir().unwrap();
        let project = d.path().join("project");
        let stage = d.path().join("stage");
        assert!(
            shell(&prepare(
                stage.join("home").to_str().unwrap(),
                project.to_str().unwrap(),
                stage.to_str().unwrap(),
                "theirs"
            ))
            .status
            .success()
        );
        assert!(
            !shell(&cleanup(
                stage.to_str().unwrap(),
                project.to_str().unwrap(),
                "ours",
                "absent-beam-test",
                true
            ))
            .status
            .success()
        );
        assert!(project.exists());
        assert!(stage.exists());
    }
}

#[cfg(test)]
mod environment_repair_tests {
    use super::*;
    use std::{
        fs,
        process::{Command, Stdio},
    };

    fn launcher(dir: &std::path::Path, repair: bool, resume: &str) -> String {
        run_script(&RunVars {
            stage: dir.to_str().unwrap(),
            cwd: dir.to_str().unwrap(),
            home: dir.to_str().unwrap(),
            handoff_head: "Original task: fix the project",
            setup: &["test -f setup-ready".into()],
            verify: &["test -f verify-ready".into()],
            reuse_setup: false,
            setup_inputs: &[],
            tools: &["beam-fixture-tool".into()],
            versions: &[crate::config::ToolVersion {
                tool: "beam-fixture-tool".into(),
                version: "2".into(),
            }],
            environment_repair: repair,
            resume_fn: resume,
            install: &ToolInstall::default(),
        })
    }

    #[test]
    fn agent_repairs_tools_setup_and_verification_before_checks_pass() {
        let d = tempfile::tempdir().unwrap();
        let script = launcher(
            d.path(),
            true,
            r#"resume() {
            printf '%s' "$1" > "$S/received"
            test "$(cat "$S/phase")" = repairing || return 10
            sh "$BEAM_REPORT" finished 'agent says done'
            test "$(cat "$S/phase")" = repairing || return 11
            mkdir -p "$HOME/.cargo/bin"
            printf '#!/bin/sh\necho tool 1.0\n' > "$HOME/.cargo/bin/beam-fixture-tool"
            chmod +x "$HOME/.cargo/bin/beam-fixture-tool"
            sh "$BEAM_CHECK" && return 12
            grep -q 'expected 2' "$S/check-report.txt" || return 13
            printf '#!/bin/sh\necho tool 2.0\n' > "$HOME/.cargo/bin/beam-fixture-tool"
            sh "$BEAM_CHECK" && return 14
            grep -q 'Setup command FAILED' "$S/check-report.txt" || return 15
            touch setup-ready
            sh "$BEAM_CHECK" && return 16
            grep -q 'Project check FAILED' "$S/check-report.txt" || return 17
            test "$(cat "$S/phase")" = repairing || return 18
            touch verify-ready
            sh "$BEAM_CHECK" || return 19
            test "$(cat "$S/phase")" = running || return 20
        }"#,
        );
        fs::write(d.path().join("run.sh"), &script).unwrap();
        fs::write(
            d.path().join("report.sh"),
            include_str!("../scripts/report.sh"),
        )
        .unwrap();
        let out = Command::new("sh")
            .arg(d.path().join("run.sh"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            fs::read_to_string(d.path().join("agent.exit"))
                .unwrap()
                .trim(),
            "0",
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        let received = fs::read_to_string(d.path().join("received")).unwrap();
        assert!(received.contains("missing tools: beam-fixture-tool"));
        assert!(received.contains("Original task: fix the project"));
        assert!(received.contains("test -f verify-ready"));
        assert!(received.contains("sh \"$BEAM_CHECK\""));
        let events = fs::read_to_string(d.path().join("events.tsv")).unwrap();
        assert!(events.contains("environment-repair-started"));
        assert!(events.contains("verification-passed"));
        for failed in [
            "check-failed\tprerequisites: missing tools: beam-fixture-tool\n",
            "check-failed\tprerequisites: beam-fixture-tool: expected 2, found 1.0. Use a matching sandbox image\n",
            "check-failed\tsetup command (exit 1): test -f setup-ready\n",
            "check-failed\tproject check (exit 1): test -f verify-ready\n",
        ] {
            assert!(events.contains(failed), "{failed:?} not in {events}");
        }
    }

    #[test]
    fn clean_agent_exit_does_not_claim_repair_passed() {
        let d = tempfile::tempdir().unwrap();
        fs::write(
            d.path().join("run.sh"),
            launcher(d.path(), true, "resume() { return 0; }"),
        )
        .unwrap();
        let out = Command::new("sh")
            .arg(d.path().join("run.sh"))
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(
            fs::read_to_string(d.path().join("phase")).unwrap().trim(),
            "needs-attention"
        );
        let events = fs::read_to_string(d.path().join("events.tsv")).unwrap();
        assert!(events.contains("agent-exited\texit=0"));
        assert!(events.contains("environment-repair-incomplete"));
        assert!(!events.contains("environment-checks-passed"));
        assert!(crate::monitor::repair_unfinished(&crate::monitor::parse(
            &events
        )));
    }

    #[test]
    fn unfinished_repair_keeps_a_shell_without_claiming_a_live_agent() {
        let d = tempfile::tempdir().unwrap();
        // Always stop this private server, including on assertion failures.
        struct Server<'a>(&'a std::path::Path);
        impl Drop for Server<'_> {
            fn drop(&mut self) {
                let _ = Command::new("tmux")
                    .arg("kill-server")
                    .env("TMUX_TMPDIR", self.0)
                    .env_remove("TMUX")
                    .output();
            }
        }
        let _server = Server(d.path());
        fs::write(
            d.path().join("run.sh"),
            launcher(d.path(), true, "resume() { return 0; }"),
        )
        .unwrap();
        let output = Command::new("tmux")
            .args(["new-session", "-d", "-s", "beam-repair-test"])
            .arg(format!(
                "sh {}",
                sh_quote(&d.path().join("run.sh").to_string_lossy())
            ))
            .env("TMUX_TMPDIR", d.path())
            .env_remove("TMUX")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !d.path().join("agent.exit").exists()
            || fs::read_to_string(d.path().join("phase"))
                .unwrap_or_default()
                .trim()
                != "needs-attention"
        {
            assert!(
                std::time::Instant::now() < deadline,
                "repair did not settle"
            );
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        let alive = Command::new("tmux")
            .args(["has-session", "-t", "beam-repair-test"])
            .env("TMUX_TMPDIR", d.path())
            .env_remove("TMUX")
            .output()
            .unwrap();
        assert!(
            alive.status.success(),
            "inspection shell disappeared: {alive:?}"
        );
        // Passing a check from the inspection shell cannot resurrect the exited agent.
        let repaired = Command::new("tmux")
            .args(["send-keys", "-t", "beam-repair-test"])
            .arg(format!(
                "printf running > {}; touch {}",
                sh_quote(&d.path().join("phase").to_string_lossy()),
                sh_quote(&d.path().join("shell-ready").to_string_lossy())
            ))
            .arg("Enter")
            .env("TMUX_TMPDIR", d.path())
            .env_remove("TMUX")
            .output()
            .unwrap();
        assert!(repaired.status.success(), "{repaired:?}");
        while !d.path().join("shell-ready").exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "inspection shell did not accept input"
            );
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        let status = Command::new("sh")
            .arg("-c")
            .arg(process_status(
                d.path().to_str().unwrap(),
                "beam-repair-test",
            ))
            .env("TMUX_TMPDIR", d.path())
            .env_remove("TMUX")
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&status.stdout).trim(), "stopped 0");
        let begin = std::time::Instant::now();
        let stopped = Command::new("sh")
            .arg("-c")
            .arg(stop_launched_agent(
                d.path().to_str().unwrap(),
                "beam-repair-test",
                true,
            ))
            .env("TMUX_TMPDIR", d.path())
            .env_remove("TMUX")
            .output()
            .unwrap();
        assert!(stopped.status.success(), "{stopped:?}");
        assert!(
            begin.elapsed() < std::time::Duration::from_secs(5),
            "inspection shell waited for an agent timeout"
        );
    }

    #[test]
    fn setup_and_verify_output_appears_while_the_command_runs_and_exit_codes_are_kept() {
        use std::io::BufRead;
        let d = tempfile::tempdir().unwrap();
        // Each command prints a line, then waits until the test has seen that line.
        let waits = |line: &str, release: &str, code: u8| {
            format!(
                "echo {line}; i=0; while [ ! -f {release} ] && [ $i -lt 100 ]; do sleep 0.05; i=$((i + 1)); done; [ -f {release} ] || touch {line}-timed-out; exit {code}"
            )
        };
        let script = run_script(&RunVars {
            stage: d.path().to_str().unwrap(),
            cwd: d.path().to_str().unwrap(),
            home: d.path().to_str().unwrap(),
            handoff_head: "Original task: fix the project",
            setup: &[waits("setup-progress", "setup-release", 0)],
            verify: &[waits("verify-progress", "verify-release", 5)],
            reuse_setup: false,
            setup_inputs: &[],
            tools: &[],
            versions: &[],
            environment_repair: false,
            resume_fn: "resume() { :; }",
            install: &ToolInstall::default(),
        });
        fs::write(d.path().join("run.sh"), script).unwrap();
        let mut child = Command::new("sh")
            .arg(d.path().join("run.sh"))
            .current_dir(d.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut lines = std::io::BufReader::new(child.stdout.take().unwrap()).lines();
        for (line, release) in [
            ("setup-progress", "setup-release"),
            ("verify-progress", "verify-release"),
        ] {
            let seen = lines.by_ref().map_while(Result::ok).any(|l| l == line);
            // Without live output, the command waits 5 s for its release, then prints.
            assert!(
                seen && !d.path().join(format!("{line}-timed-out")).exists(),
                "{line} did not appear while its command ran"
            );
            fs::write(d.path().join(release), "").unwrap();
        }
        let rest: Vec<String> = lines.map_while(Result::ok).collect();
        child.wait().unwrap();
        assert!(
            !rest.iter().any(|l| l.contains("-progress")),
            "output was printed twice: {rest:?}"
        );
        let report = fs::read_to_string(d.path().join("check-report.txt")).unwrap();
        assert!(report.contains("Setup command succeeded"), "{report}");
        assert!(report.contains("Project check FAILED (exit 5)"), "{report}");
        let log = fs::read_to_string(d.path().join("setup.log")).unwrap();
        assert!(
            log.contains("setup-progress") && log.contains("verify-progress"),
            "{log}"
        );
    }

    #[test]
    fn shell_failure_keeps_manual_recovery_and_does_not_launch_agent() {
        let d = tempfile::tempdir().unwrap();
        fs::write(
            d.path().join("run.sh"),
            launcher(d.path(), false, "resume() { touch agent-launched; }"),
        )
        .unwrap();
        let out = Command::new("sh")
            .arg(d.path().join("run.sh"))
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert_eq!(
            fs::read_to_string(d.path().join("phase")).unwrap().trim(),
            "needs-attention"
        );
        assert!(!d.path().join("agent-launched").exists());
        assert!(
            fs::read_to_string(d.path().join("setup.log"))
                .unwrap()
                .contains("missing tools")
        );
    }

    #[test]
    fn old_saved_launcher_can_be_upgraded_once_without_changing_its_context() {
        let d = tempfile::tempdir().unwrap();
        let fresh = launcher(
            d.path(),
            true,
            "resume() { printf '%s' \"$1\" > received; }",
        );
        let (prefix, _) = fresh
            .trim_start_matches(REPAIR_LAUNCHER)
            .split_once("PREREQUISITES=")
            .unwrap();
        let old = format!(
            "{prefix}# ABOUTME: Runs setup and project checks, recording evidence and elapsed time.\nexit 99\n"
        );
        let upgraded = upgrade_run_script(&old, "echo missing-cargo; exit 4", true)
            .unwrap()
            .unwrap();
        assert!(upgrade_run_script(&upgraded, "", true).unwrap().is_none());
        fs::write(d.path().join("run.sh"), upgraded).unwrap();
        let out = Command::new("sh")
            .arg(d.path().join("run.sh"))
            .output()
            .unwrap();
        assert!(out.status.success());
        let received = fs::read_to_string(d.path().join("received")).unwrap();
        assert!(received.contains("Original task: fix the project"));
        assert!(received.contains("missing-cargo"));
        assert!(received.contains("Repair the sandbox environment"));
    }
}

#[cfg(test)]
mod stop_tests {
    use super::*;
    use std::process::Command;

    /// Run a script against a private tmux server so that tests never touch the user's sessions.
    fn tmux_sh(socket_dir: &std::path::Path, script: &str) -> std::process::Output {
        Command::new("sh")
            .arg("-c")
            .arg(script)
            .env("TMUX_TMPDIR", socket_dir)
            .env_remove("TMUX")
            .output()
            .unwrap()
    }

    #[test]
    fn stopping_an_interactive_shell_does_not_wait_for_the_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let started = tmux_sh(dir.path(), "tmux new-session -d -s beam-test 'sh -i'");
        assert!(started.status.success(), "{started:?}");
        let begin = std::time::Instant::now();
        let out = tmux_sh(dir.path(), &stop_agent("beam-test", false));
        let took = begin.elapsed();
        assert!(out.status.success(), "{out:?}");
        let alive = tmux_sh(dir.path(), "tmux has-session -t beam-test");
        let _ = tmux_sh(dir.path(), "tmux kill-server");
        assert!(
            !alive.status.success(),
            "the shell session is still running"
        );
        assert!(took.as_secs() < 5, "stopping a shell took {took:?}");
    }

    #[test]
    fn graceful_stop_lets_the_process_exit_after_ctrl_c() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("saved");
        let program = format!(
            "trap 'touch {}; exit 0' INT; while :; do sleep 0.1; done",
            sh_quote(&marker.to_string_lossy())
        );
        let started = tmux_sh(
            dir.path(),
            &format!(
                "tmux new-session -d -s beam-test {}",
                sh_quote(&format!("sh -c {}", sh_quote(&program)))
            ),
        );
        assert!(started.status.success(), "{started:?}");
        let begin = std::time::Instant::now();
        let out = tmux_sh(dir.path(), &stop_agent("beam-test", true));
        let took = begin.elapsed();
        let alive = tmux_sh(dir.path(), "tmux has-session -t beam-test");
        let _ = tmux_sh(dir.path(), "tmux kill-server");
        assert!(out.status.success(), "{out:?}");
        assert!(!alive.status.success(), "the session is still running");
        assert!(
            marker.exists(),
            "the process did not handle Ctrl-C before it ended"
        );
        assert!(took.as_secs() < 5, "graceful stop took {took:?}");
    }
}

#[cfg(test)]
mod tool_install_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process::Command;

    fn executable(path: &Path, body: &str) {
        std::fs::write(path, format!("#!/bin/sh\n{body}")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A Node.js release with stub `node` and `npm`, served by a stub `curl` that logs each URL.
    fn fixture(d: &Path) -> std::path::PathBuf {
        fixture_with_node(d, "echo v22.9.9\n")
    }

    fn fixture_with_node(d: &Path, node: &str) -> std::path::PathBuf {
        let stubs = d.join("stubs");
        let release = d.join("node-v22.9.9-linux-x64");
        for sub in ["bin", "lib", "include", "share"] {
            std::fs::create_dir_all(release.join(sub)).unwrap();
        }
        executable(&release.join("bin/node"), node);
        executable(
            &release.join("bin/npm"),
            &format!(
                "echo \"$*\" > {}\n",
                sh_quote(&d.join("npm-args").to_string_lossy())
            ),
        );
        std::fs::create_dir_all(&stubs).unwrap();
        let tarball = d.join("node-v22.9.9-linux-x64.tar.gz");
        assert!(
            Command::new("tar")
                .args(["czf", tarball.to_str().unwrap(), "-C", d.to_str().unwrap()])
                .arg("node-v22.9.9-linux-x64")
                .status()
                .unwrap()
                .success()
        );
        // The script PATH has only the stubs and system directories. Find a real tool for the stub.
        let real = Command::new("sh")
            .args(["-c", "command -v sha256sum || command -v shasum"])
            .output()
            .unwrap();
        let real = String::from_utf8_lossy(&real.stdout).trim().to_string();
        let flags = if real.ends_with("shasum") {
            " -a 256"
        } else {
            ""
        };
        executable(
            &stubs.join("sha256sum"),
            &format!("exec {}{flags} \"$@\"\n", sh_quote(&real)),
        );
        let sum = Command::new("sh")
            .arg("-c")
            .arg(format!(
                "PATH={}:$PATH sha256sum {}",
                sh_quote(stubs.to_str().unwrap()),
                sh_quote(tarball.to_str().unwrap())
            ))
            .output()
            .unwrap();
        let sum = String::from_utf8_lossy(&sum.stdout);
        let sum = sum.split_whitespace().next().unwrap();
        std::fs::write(
            d.join("SHASUMS256.txt"),
            format!(
                "{sum}  node-v22.9.9-linux-x64.tar.gz\n{sum}  node-v22.9.9-darwin-x64.tar.gz\n"
            ),
        )
        .unwrap();
        let fixture = sh_quote(d.to_str().unwrap());
        executable(
            &stubs.join("curl"),
            &format!(
                r#"out=''; url=''
while [ $# -gt 0 ]; do case "$1" in -o) out=$2; shift 2 ;; -*) shift ;; *) url=$1; shift ;; esac; done
echo "$url" >> {fixture}/urls
case "$url" in
  *SHASUMS256.txt) cat {fixture}/SHASUMS256.txt ;;
  *.tar.gz) cp {fixture}/node-v22.9.9-linux-x64.tar.gz "$out" ;;
  *) exit 22 ;;
esac
"#
            ),
        );
        executable(&stubs.join("uname"), "echo x86_64\n");
        stubs
    }

    fn install(d: &Path, stubs: &Path, tools: &str, enabled: bool, node: &str) -> String {
        let prefix = d.join("prefix");
        let script = format!(
            "{}{}install_tools",
            with_vars(
                &[
                    ("TOOLS", tools),
                    ("INSTALL_TOOLS", if enabled { "yes" } else { "no" }),
                    ("NODE_VERSION", node),
                    ("PNPM_VERSION", "11.11.0"),
                    ("UV_VERSION", ""),
                    ("RUST_VERSION", ""),
                    ("TOOLS_PREFIX", prefix.to_str().unwrap()),
                ],
                ""
            ),
            include_str!("../scripts/tools.sh")
        );
        // The system directories have no node; the installed one is found below the prefix.
        let path = format!("{}:{}/bin:/usr/bin:/bin", stubs.display(), prefix.display());
        let out = Command::new("sh")
            .args(["-c", &script])
            .env("PATH", path)
            .env("HOME", d)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    #[test]
    fn pnpm_installs_verified_node_then_the_requested_pnpm_and_skips_unknown_tools() {
        let d = tempfile::tempdir().unwrap();
        let stubs = fixture(d.path());
        let out = install(d.path(), &stubs, "git\npnpm\nzig", true, "22");
        assert!(out.contains("beam: installing pnpm"), "{out}");
        assert!(!out.contains("zig"), "{out}");
        let urls = std::fs::read_to_string(d.path().join("urls")).unwrap();
        assert_eq!(
            urls,
            "https://nodejs.org/dist/latest-v22.x/SHASUMS256.txt\nhttps://nodejs.org/dist/latest-v22.x/node-v22.9.9-linux-x64.tar.gz\n"
        );
        let prefix = d.path().join("prefix");
        assert!(prefix.join("bin/node").is_file(), "{out}");
        assert!(prefix.join("lib").is_dir());
        assert_eq!(
            std::fs::read_to_string(d.path().join("npm-args"))
                .unwrap()
                .trim(),
            format!("install -g --prefix {} pnpm@11.11.0", prefix.display())
        );
    }

    #[test]
    fn node_that_does_not_start_gets_libatomic_from_apt() {
        let d = tempfile::tempdir().unwrap();
        let marker = d.path().join("libatomic-installed");
        let marker = sh_quote(marker.to_str().unwrap());
        let stubs = fixture_with_node(
            d.path(),
            &format!("test -f {marker} || exit 127\necho v22.9.9\n"),
        );
        executable(&stubs.join("id"), "echo 0\n");
        executable(
            &stubs.join("apt-get"),
            &format!(
                "echo \"$*\" >> {}\ncase \"$*\" in *libatomic1*) touch {marker} ;; esac\n",
                sh_quote(d.path().join("apt-args").to_str().unwrap())
            ),
        );
        let out = install(d.path(), &stubs, "node", true, "22");
        assert!(!out.contains("does not start"), "{out}");
        let apt = std::fs::read_to_string(d.path().join("apt-args")).unwrap();
        assert!(apt.contains("install -y -qq libatomic1"), "{apt}");
    }

    #[test]
    fn existing_node_with_another_major_version_stays_and_is_reported() {
        let d = tempfile::tempdir().unwrap();
        let stubs = fixture(d.path());
        executable(&stubs.join("node"), "echo v22.23.3\n");
        let out = install(d.path(), &stubs, "pnpm", false, "26");
        assert!(out.is_empty(), "disabled installs print nothing: {out}");
        let out = install(d.path(), &stubs, "node", true, "26");
        assert!(
            out.contains("the sandbox has Node.js 22.23.3; the plan asked for 26"),
            "{out}"
        );
        assert!(!d.path().join("urls").exists(), "Node.js was downloaded");
        let same = install(d.path(), &stubs, "node", true, "22");
        assert!(same.is_empty(), "{same}");
    }

    #[test]
    fn exact_node_versions_use_their_release_directory() {
        let d = tempfile::tempdir().unwrap();
        let stubs = fixture(d.path());
        install(d.path(), &stubs, "node", true, "22.9.9");
        let urls = std::fs::read_to_string(d.path().join("urls")).unwrap();
        assert!(
            urls.starts_with("https://nodejs.org/dist/v22.9.9/SHASUMS256.txt\n"),
            "{urls}"
        );
    }

    #[test]
    fn checksum_mismatch_installs_nothing() {
        let d = tempfile::tempdir().unwrap();
        let stubs = fixture(d.path());
        let sums = std::fs::read_to_string(d.path().join("SHASUMS256.txt")).unwrap();
        std::fs::write(
            d.path().join("SHASUMS256.txt"),
            sums.replacen(&sums[..8], "00000000", 1),
        )
        .unwrap();
        let out = install(d.path(), &stubs, "pnpm", true, "22");
        assert!(out.contains("checksum did not match"), "{out}");
        assert!(out.contains("could not install pnpm"), "{out}");
        assert!(!d.path().join("prefix/bin/node").exists());
        assert!(!d.path().join("npm-args").exists());
    }

    #[test]
    fn disabled_installation_downloads_nothing() {
        let d = tempfile::tempdir().unwrap();
        let stubs = fixture(d.path());
        let out = install(d.path(), &stubs, "pnpm\nnode", false, "22");
        assert!(out.is_empty(), "{out}");
        assert!(!d.path().join("urls").exists());
    }

    #[test]
    fn plan_line_lists_the_tools_to_install() {
        let mut install = ToolInstall {
            enabled: true,
            node: "22".into(),
            pnpm: "11.11.0".into(),
            ..Default::default()
        };
        assert_eq!(
            install.describe().unwrap(),
            "installs Node.js 22 and pnpm 11.11.0 if missing; existing versions stay"
        );
        install.enabled = false;
        assert_eq!(install.describe(), None);
        assert_eq!(
            ToolInstall {
                enabled: true,
                ..Default::default()
            }
            .describe(),
            None
        );
    }
}
