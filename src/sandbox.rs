// ABOUTME: Sandbox providers: Docker (local or on an SSH host), plain SSH hosts, Steel computers, and Daytona sandboxes.
// ABOUTME: All work in a sandbox goes through "sh -c <script>", so one small interface serves all providers.

use crate::util::{run, sh_join, sh_quote};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::path::Path;
use std::process::{Command, Stdio};

/// Where the user wants the session to go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Docker on this machine, or on an SSH host when `ssh_host` is set.
    Docker {
        ssh_host: Option<String>,
    },
    /// An existing machine that beam reaches with ssh.
    Ssh {
        host: String,
    },
    /// A new Steel computer, or a copy of a Steel checkpoint.
    Steel {
        checkpoint: Option<String>,
    },
    Daytona {
        snapshot: Option<String>,
    },
}

/// What a new sandbox needs to know.
pub struct CreateOpts<'a> {
    pub name: &'a str,
    pub image: &'a str,
    pub session_id: &'a str,
    pub timeout_secs: u64,
    pub receipt: &'a Path,
    /// Docker: mount the shared package cache volume.
    pub cache: bool,
}

/// The Docker volume that keeps package downloads between sandboxes.
pub const CACHE_VOLUME: &str = "beam-cache";

/// Package managers keep their downloads below this directory in the cache volume.
const CACHE_ENV: [(&str, &str); 8] = [
    // pnpm 11 and later read pnpm_config_*; earlier versions read npm_config_*.
    ("pnpm_config_store_dir", "/var/cache/beam/pnpm"),
    ("npm_config_store_dir", "/var/cache/beam/pnpm"),
    // Package metadata. Without it, each sandbox asks the registry about every package again.
    ("pnpm_config_cache_dir", "/var/cache/beam/pnpm-cache"),
    ("npm_config_cache", "/var/cache/beam/npm"),
    ("YARN_CACHE_FOLDER", "/var/cache/beam/yarn"),
    ("BUN_INSTALL_CACHE_DIR", "/var/cache/beam/bun"),
    ("UV_CACHE_DIR", "/var/cache/beam/uv"),
    ("PIP_CACHE_DIR", "/var/cache/beam/pip"),
];

fn docker_run_args(name: &str, session_id: &str, image: &str, cache: bool) -> Vec<String> {
    let mut args: Vec<String> = [
        "docker",
        "run",
        "-d",
        "--name",
        name,
        "--label",
        "beam=1",
        "--label",
        &format!("beam.session={session_id}"),
        "-u",
        "0",
    ]
    .map(String::from)
    .to_vec();
    if cache {
        args.extend(["-v".into(), format!("{CACHE_VOLUME}:/var/cache/beam")]);
        for (name, value) in CACHE_ENV {
            args.extend(["-e".into(), format!("{name}={value}")]);
        }
    }
    args.extend(["--entrypoint", "sleep", image, "infinity"].map(String::from));
    args
}

impl Target {
    pub fn parse(s: &str) -> Result<Target> {
        let host = |h: &str| -> Result<String> {
            let h = h.trim_end_matches('/');
            if h.is_empty() || h.starts_with('-') || h.chars().any(char::is_whitespace) {
                bail!("target {s:?} has an invalid host");
            }
            Ok(h.to_string())
        };
        if s == "docker" {
            Ok(Target::Docker { ssh_host: None })
        } else if let Some(h) = s.strip_prefix("docker+ssh://") {
            Ok(Target::Docker {
                ssh_host: Some(host(h)?),
            })
        } else if let Some(h) = s.strip_prefix("ssh://") {
            Ok(Target::Ssh { host: host(h)? })
        } else if s == "daytona" {
            Ok(Target::Daytona { snapshot: None })
        } else if let Some(snapshot) = s.strip_prefix("daytona:") {
            if snapshot.is_empty() {
                bail!("target {s:?} has no snapshot name");
            }
            Ok(Target::Daytona {
                snapshot: Some(snapshot.into()),
            })
        } else if s == "steel" {
            Ok(Target::Steel { checkpoint: None })
        } else if let Some(c) = s.strip_prefix("steel:") {
            if c.is_empty() {
                bail!("target {s:?} has no checkpoint id");
            }
            Ok(Target::Steel {
                checkpoint: Some(c.to_string()),
            })
        } else {
            bail!(
                "unknown target {s:?}. Use docker, docker+ssh://HOST, ssh://HOST, steel, steel:CHECKPOINT, daytona, or daytona:SNAPSHOT"
            )
        }
    }

    /// Start a sandbox. For SSH, the host already exists, so this only checks that it answers.
    pub fn create(&self, o: &CreateOpts) -> Result<Sandbox> {
        let (name, image, session_id) = (o.name, o.image, o.session_id);
        match self {
            Target::Docker { ssh_host } => {
                if let Ok(label) = run(&mut host_cmd(
                    ssh_host.as_deref(),
                    &[
                        "docker",
                        "inspect",
                        "--format",
                        "{{index .Config.Labels \"beam.session\"}}",
                        name,
                    ],
                    false,
                )) {
                    if label != session_id {
                        bail!("container {name} belongs to another transfer");
                    }
                    return Ok(Sandbox::Docker {
                        ssh_host: ssh_host.clone(),
                        container: name.into(),
                    });
                }
                let args = docker_run_args(name, session_id, image, o.cache);
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                let mut cmd = host_cmd(ssh_host.as_deref(), &args, false);
                run(&mut cmd).map_err(|e| {
                    if e.to_string().contains("Unable to find image")
                        || e.to_string().contains("pull access denied")
                    {
                        e.context(if image == crate::config::DEFAULT_IMAGE {
                            format!("image {image} is not there. Build it: beam build-image")
                        } else {
                            format!(
                                "image {image} is not there. Build or pull it on the Docker host"
                            )
                        })
                    } else {
                        e
                    }
                })?;
                Ok(Sandbox::Docker {
                    ssh_host: ssh_host.clone(),
                    container: name.to_string(),
                })
            }
            Target::Ssh { host } => {
                let sb = Sandbox::Ssh { host: host.clone() };
                sb.exec("true")
                    .with_context(|| format!("cannot reach {host} with ssh"))?;
                Ok(sb)
            }
            Target::Daytona { snapshot } => Ok(Sandbox::Daytona {
                id: crate::daytona::create(snapshot.as_deref(), o)?,
            }),
            Target::Steel { checkpoint } => {
                let id = crate::steel::create(checkpoint.as_deref(), o.timeout_secs, o.receipt)?;
                Ok(Sandbox::Steel { id })
            }
        }
    }
}

/// A running sandbox. It is saved in the beam state file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Sandbox {
    Docker {
        ssh_host: Option<String>,
        container: String,
    },
    Ssh {
        host: String,
    },
    Steel {
        id: String,
    },
    Daytona {
        id: String,
    },
}

/// A command that runs `argv` on this machine, or on `ssh_host` through ssh.
fn host_cmd(ssh_host: Option<&str>, argv: &[&str], tty: bool) -> Command {
    match ssh_host {
        None => {
            let mut c = Command::new(argv[0]);
            c.args(&argv[1..]);
            c
        }
        Some(h) => {
            let mut c = Command::new("ssh");
            c.arg(if tty { "-t" } else { "-T" })
                .arg(h)
                .arg(sh_join(argv));
            c
        }
    }
}

impl Sandbox {
    pub fn describe(&self) -> String {
        match self {
            Sandbox::Docker {
                ssh_host: None,
                container,
            } => format!("docker:{container}"),
            Sandbox::Docker {
                ssh_host: Some(h),
                container,
            } => format!("docker+ssh://{h}:{container}"),
            Sandbox::Ssh { host } => format!("ssh://{host}"),
            Sandbox::Steel { id } => format!("steel:{id}"),
            Sandbox::Daytona { id } => format!("daytona:{id}"),
        }
    }

    fn command(&self, script: &str, stdin: bool, tty: bool) -> Result<Command> {
        Ok(match self {
            Sandbox::Daytona { id } => return crate::daytona::command(id, script, tty),
            Sandbox::Docker {
                ssh_host,
                container,
            } => {
                let flag = match (stdin, tty) {
                    (_, true) => "-it",
                    (true, false) => "-i",
                    (false, false) => "",
                };
                let mut argv = vec!["docker", "exec"];
                if !flag.is_empty() {
                    argv.push(flag);
                }
                argv.extend([container.as_str(), "sh", "-c", script]);
                host_cmd(ssh_host.as_deref(), &argv, tty)
            }
            Sandbox::Ssh { host } => {
                let mut c = Command::new("ssh");
                c.arg(if tty { "-t" } else { "-T" })
                    .arg(host)
                    .arg(format!("sh -c {}", sh_quote(script)));
                c
            }
            Sandbox::Steel { id } => {
                // Only for display and tests: the Steel methods below do not use this command.
                let mut c = crate::steel::steel();
                c.args(["computer", "ssh", id, "--", "sh", "-c", script]);
                c
            }
        })
    }

    /// Make sure the sandbox can run commands (a Steel computer can pause itself).
    pub fn wake(&self) -> Result<()> {
        match self {
            Sandbox::Steel { id } => crate::steel::wake(id),
            Sandbox::Daytona { id } => crate::daytona::wake(id),
            _ => Ok(()),
        }
    }

    pub fn process_status(&self, stage: &str, tmux: &str) -> Result<String> {
        self.exec_when_running(&crate::remote::process_status(stage, tmux))
    }

    /// Run a script only when the cloud sandbox runs. Otherwise return its provider status,
    /// for example "paused". This does not wake the sandbox.
    pub fn exec_when_running(&self, script: &str) -> Result<String> {
        if let Self::Steel { id } = self {
            let status = crate::steel::status(id)?;
            if status != "running" {
                return Ok(status);
            }
        }
        if let Self::Daytona { id } = self {
            let status = crate::daytona::status(id)?;
            if status != "running" {
                return Ok(status);
            }
        }
        self.exec(script)
    }

    /// Run a script in the sandbox. Returns stdout.
    pub fn exec(&self, script: &str) -> Result<String> {
        if let Sandbox::Steel { id } = self {
            return crate::steel::exec(id, script);
        }
        let mut c = self.command(script, false, false)?;
        c.stdin(Stdio::null());
        run(&mut c)
    }

    /// Run a script with `data` on stdin.
    pub fn exec_input(&self, script: &str, data: &[u8]) -> Result<String> {
        use std::io::Write;
        if let Sandbox::Steel { id } = self {
            let dir = crate::up::tempdir()?;
            let tmp = dir.path().join("stdin");
            std::fs::write(&tmp, data)?;
            let out = crate::steel::ssh_output(id, script, Stdio::from(File::open(&tmp)?));

            return out.map(|o| String::from_utf8_lossy(&o).trim().to_string());
        }
        let mut c = self.command(script, true, false)?;
        c.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn().context("cannot start the sandbox command")?;
        let mut stdin = child.stdin.take().context("no stdin")?;
        let data = data.to_vec();
        let writer = std::thread::spawn(move || stdin.write_all(&data));
        let out = child.wait_with_output()?;
        writer
            .join()
            .map_err(|_| anyhow::anyhow!("upload writer stopped"))??;
        if !out.status.success() {
            bail!(
                "sandbox command failed ({}): {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// Run a script with the content of a local file on stdin.
    pub fn exec_file(&self, script: &str, file: &Path) -> Result<String> {
        let f = File::open(file).with_context(|| format!("cannot open {}", file.display()))?;
        if let Sandbox::Steel { id } = self {
            let out = crate::steel::ssh_output(id, script, Stdio::from(f))?;
            return Ok(String::from_utf8_lossy(&out).trim().to_string());
        }
        let mut c = self.command(script, true, false)?;
        c.stdin(Stdio::from(f));
        run(&mut c)
    }

    /// Stream a download to disk. Rename the partial file only after success.
    pub fn download(&self, script: &str, path: &Path) -> Result<()> {
        let parent = path.parent().context("download has no parent")?;
        crate::util::private_dir(parent)?;
        let tmp = tempfile::NamedTempFile::new_in(parent)?;
        if let Sandbox::Steel { id } = self {
            crate::steel::download(id, script, tmp.as_file())?;
        } else {
            let out = self
                .command(script, false, false)?
                .stdin(Stdio::null())
                .stdout(tmp.as_file().try_clone()?)
                .stderr(Stdio::piped())
                .output()?;
            if !out.status.success() {
                bail!("download failed: {}", String::from_utf8_lossy(&out.stderr));
            }
        }
        tmp.as_file().sync_all()?;
        tmp.persist(path).map_err(|e| e.error)?;
        Ok(())
    }

    /// Connect this terminal to a script in the sandbox (for example "tmux attach").
    pub fn interactive(&self, script: &str) -> Result<()> {
        if let Sandbox::Steel { id } = self {
            return crate::steel::interactive(id, script);
        }
        let status = self
            .command(script, true, true)?
            .status()
            .context("cannot start the terminal")?;
        if !status.success() {
            bail!("the sandbox terminal stopped with {status}");
        }
        Ok(())
    }

    /// Delete the sandbox. For SSH hosts, `cleanup` removes what beam put there.
    pub fn destroy(&self, cleanup: &str, owner: &str) -> Result<()> {
        match self {
            Sandbox::Docker {
                ssh_host,
                container,
            } => {
                match run(&mut host_cmd(
                    ssh_host.as_deref(),
                    &[
                        "docker",
                        "inspect",
                        "--format",
                        "{{index .Config.Labels \"beam.session\"}}",
                        container,
                    ],
                    false,
                )) {
                    Ok(label) if label == owner => {}
                    Ok(_) => bail!(
                        "container {container} is not owned by this transfer; nothing was deleted"
                    ),
                    Err(e) if e.to_string().contains("No such") => return Ok(()),
                    Err(e) => return Err(e),
                }
                let out = run(&mut host_cmd(
                    ssh_host.as_deref(),
                    &["docker", "rm", "-f", container],
                    false,
                ));
                match out {
                    Ok(_) => Ok(()),
                    Err(e) if e.to_string().contains("No such container") => Ok(()),
                    Err(e) => Err(e),
                }
            }
            Sandbox::Ssh { .. } => self.exec(cleanup).map(|_| ()),
            Sandbox::Steel { id } => crate::steel::delete(id),
            Sandbox::Daytona { id } => crate::daytona::delete(id, owner),
        }
    }
}

impl Target {
    pub fn login_home(&self) -> Result<Option<String>> {
        if let Target::Ssh { host } = self {
            return Ok(Some(
                Sandbox::Ssh { host: host.clone() }.exec("printf '%s' \"$HOME\"")?,
            ));
        }
        Ok(None)
    }

    /// Read-only checks shared by doctor and upload. Run before any transfer resource is created.
    pub fn preflight(
        &self,
        image: &str,
        tools: &[String],
        versions: &[crate::config::ToolVersion],
        project: &Path,
    ) -> Result<()> {
        let script = prerequisite_script(tools, versions);
        match self {
            Target::Docker { ssh_host } => {
                run(&mut host_cmd(
                    ssh_host.as_deref(),
                    &["docker", "info", "--format", "{{.ServerVersion}}"],
                    false,
                ))
                .context("Docker is unavailable. Start Docker or choose another --to target")?;
                let built = run(&mut host_cmd(
                    ssh_host.as_deref(),
                    &["docker", "image", "inspect", "--format", "{{index .Config.Labels \"beam.built\"}}", image],
                    false,
                ))
                .with_context(|| format!("image {image} is missing. Run `beam build-image` to build the default image, or set [sandbox] image"))?;
                let rebuild = match ssh_host {
                    Some(host) => format!("beam build-image --to docker+ssh://{host}"),
                    None => "beam build-image".into(),
                };
                if image == crate::config::DEFAULT_IMAGE
                    && let Some(warning) =
                        image_age_warning(image, &built, crate::util::now_unix(), &rebuild)
                {
                    crate::ui::warn(warning);
                }
                run(&mut host_cmd(ssh_host.as_deref(), &["docker", "run", "--rm", "--entrypoint", "sh", image, "-c", &script], false))
                    .context("sandbox prerequisites are missing. Add the tools to your image, or set [sandbox] setup explicitly")?;
            }
            Target::Ssh { host } => {
                let sb = Sandbox::Ssh { host: host.clone() };
                sb.exec(&script)?;
                sb.exec(&format!("test ! -e {} || {{ echo 'destination exists; choose an empty destination host' >&2; exit 3; }}", sh_quote(&project.to_string_lossy())))?;
            }
            Target::Daytona { .. } => crate::daytona::preflight()?,
            Target::Steel { .. } => {
                crate::steel::preflight()?;
                // Tools depend on the checkpoint and are checked after bootstrap, before upload.
            }
        }
        Ok(())
    }

    pub fn build_image(&self, image: &str) -> Result<()> {
        let Target::Docker { ssh_host } = self else {
            bail!("--build-image is only available for Docker targets");
        };
        use std::io::Write;
        let mut c = host_cmd(
            ssh_host.as_deref(),
            &[
                "docker",
                "build",
                "--network",
                "host",
                "--build-arg",
                &format!("BEAM_BUILT={}", crate::util::now_unix()),
                "-t",
                image,
                "-",
            ],
            false,
        );
        let mut child = c
            .stdin(Stdio::piped())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()?;
        child
            .stdin
            .take()
            .context("no build stdin")?
            .write_all(include_bytes!("../images/base/Dockerfile"))?;
        if !child.wait()?.success() {
            bail!("image build failed");
        }
        Ok(())
    }
}

/// The default image gets old: Claude Code and tools in it do not update by themselves.
const IMAGE_MAX_AGE_DAYS: u64 = 30;

/// A warning when the default image is old or has no build time label. `built` is the label value.
fn image_age_warning(image: &str, built: &str, now: u64, command: &str) -> Option<String> {
    let rebuild = format!("Rebuild it with: {command}");
    match built.trim().parse::<u64>() {
        Ok(at) if at > 0 => {
            let days = now.saturating_sub(at) / 86_400;
            (days > IMAGE_MAX_AGE_DAYS).then(|| {
                format!("{image} was built {days} days ago; Claude Code and its tools may be out of date. {rebuild}")
            })
        }
        _ => Some(format!(
            "{image} was built by an older Beam, without pinned tool versions. {rebuild}"
        )),
    }
}

pub fn prerequisite_script(tools: &[String], versions: &[crate::config::ToolVersion]) -> String {
    let mut script = format!(
        r#"export PATH="$PATH:$HOME/.cargo/bin:$HOME/.local/bin:$HOME/.npm-global/bin:/usr/local/bin"
missing=''
for t in {}; do command -v "$t" >/dev/null 2>&1 || missing="$missing $t"; done
if [ -n "$missing" ]; then echo "missing tools:$missing" >&2; exit 4; fi
git --version
"#,
        sh_join(tools)
    );
    for pin in versions {
        let command = if pin.tool == "go" {
            "go version".into()
        } else {
            format!("{} --version", sh_quote(&pin.tool))
        };
        script.push_str(&format!(r#"actual=$({command} | sed -n 's/^[^0-9]*\([0-9][0-9.]*\).*$/\1/p')
case "$actual" in {version}|{version}.*) ;; *) echo "{tool}: expected {version}, found $actual. Use a matching sandbox image" >&2; exit 4;; esac
"#, tool=pin.tool, version=pin.version));
    }
    script
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_or_unlabeled_default_images_get_a_rebuild_warning() {
        let day = 86_400;
        let now = 100 * day;
        let age =
            |built: &str| image_age_warning("beam-base:latest", built, now, "beam build-image");
        assert_eq!(age(&(now - 3 * day).to_string()), None);
        let old = age(&(now - 45 * day).to_string()).unwrap();
        assert!(
            old.contains("built 45 days ago") && old.ends_with("Rebuild it with: beam build-image"),
            "{old}"
        );
        // Docker prints "<no value>" for a missing label.
        for missing in ["<no value>", "", "0"] {
            let warning = age(missing).unwrap();
            assert!(warning.contains("older Beam"), "{warning}");
        }
    }

    #[test]
    fn docker_cache_mounts_the_shared_volume_and_points_package_managers_at_it() {
        let cached = docker_run_args("beam-1", "1", "beam-base:latest", true);
        let text = cached.join(" ");
        assert!(text.contains("-v beam-cache:/var/cache/beam"), "{text}");
        for name in [
            "pnpm_config_store_dir",
            "npm_config_store_dir",
            "npm_config_cache",
            "UV_CACHE_DIR",
        ] {
            assert!(
                text.contains(&format!("-e {name}=/var/cache/beam/")),
                "{text}"
            );
        }
        // The image and its command stay last.
        assert_eq!(cached[cached.len() - 2..], ["beam-base:latest", "infinity"]);
        let plain = docker_run_args("beam-1", "1", "beam-base:latest", false).join(" ");
        assert!(
            !plain.contains("beam-cache") && !plain.contains(" -e "),
            "{plain}"
        );
    }

    #[test]
    fn prerequisite_versions_match_complete_components() {
        let d = tempfile::tempdir().unwrap();
        let node = d.path().join("node");
        std::fs::write(&node, "#!/bin/sh\necho v22.14.0\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = format!("{}:{}", d.path().display(), std::env::var("PATH").unwrap());
        for (version, good) in [("22", true), ("22.14.0", true), ("2", false), ("20", false)] {
            let script = prerequisite_script(
                &["node".into()],
                &[crate::config::ToolVersion {
                    tool: "node".into(),
                    version: version.into(),
                }],
            );
            let out = Command::new("sh")
                .args(["-c", &script])
                .env("PATH", &path)
                .output()
                .unwrap();
            assert_eq!(
                out.status.success(),
                good,
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    #[test]
    fn parses_targets() {
        assert_eq!(
            Target::parse("docker").unwrap(),
            Target::Docker { ssh_host: None }
        );
        assert_eq!(
            Target::parse("docker+ssh://agent").unwrap(),
            Target::Docker {
                ssh_host: Some("agent".into())
            }
        );
        assert_eq!(
            Target::parse("ssh://me@box/").unwrap(),
            Target::Ssh {
                host: "me@box".into()
            }
        );
        assert!(Target::parse("ssh://").is_err());
        assert_eq!(
            Target::parse("steel").unwrap(),
            Target::Steel { checkpoint: None }
        );
        assert_eq!(
            Target::parse("steel:ckp_1").unwrap(),
            Target::Steel {
                checkpoint: Some("ckp_1".into())
            }
        );
        assert!(Target::parse("steel:").is_err());
        assert_eq!(
            Target::parse("daytona").unwrap(),
            Target::Daytona { snapshot: None }
        );
        assert_eq!(
            Target::parse("daytona:my-snapshot").unwrap(),
            Target::Daytona {
                snapshot: Some("my-snapshot".into())
            }
        );
        assert!(Target::parse("daytona:").is_err());
        assert!(Target::parse("e2b").is_err());
    }

    #[test]
    fn builds_docker_over_ssh_command() {
        let sb = Sandbox::Docker {
            ssh_host: Some("agent".into()),
            container: "beam-1".into(),
        };
        let c = sb.command("echo 'hi'", true, false).unwrap();
        let args: Vec<String> = c
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(c.get_program(), "ssh");
        assert_eq!(
            args,
            vec![
                "-T",
                "agent",
                r"docker exec -i beam-1 sh -c 'echo '\''hi'\'''"
            ]
        );
    }

    #[test]
    fn state_json_round_trip() {
        let cloud = Sandbox::Daytona {
            id: "sandbox-1".into(),
        };
        let cloud_json = serde_json::to_string(&cloud).unwrap();
        assert_eq!(cloud_json, r#"{"kind":"daytona","id":"sandbox-1"}"#);
        assert_eq!(serde_json::from_str::<Sandbox>(&cloud_json).unwrap(), cloud);
        let sb = Sandbox::Ssh { host: "box".into() };
        let j = serde_json::to_string(&sb).unwrap();
        assert_eq!(j, r#"{"kind":"ssh","host":"box"}"#);
        assert_eq!(serde_json::from_str::<Sandbox>(&j).unwrap(), sb);
    }
}
