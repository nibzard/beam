// ABOUTME: A single transfer plan supplies the preview, validation, and archive inputs.
use crate::{
    agent::{self, Adapter, Session},
    config::{Config, DEFAULT_MAX_FILE_SIZE, UserConfig},
    git,
    sandbox::Target,
    ui::{self, Hue},
    util,
};
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

pub struct Plan {
    pub root: PathBuf,
    pub cwd: PathBuf,
    pub home: PathBuf,
    pub config: Config,
    pub target: String,
    pub target_source: TargetSource,
    pub session: Option<Session>,
    pub agent: &'static dyn Adapter,
    pub extras: Vec<String>,
    pub return_extras: Vec<String>,
    pub extra_files: Vec<String>,
    pub agent_files: Vec<String>,
    pub defaults: Vec<String>,
    pub env: Vec<(String, String)>,
    pub setup: Vec<String>,
    pub tools: Vec<String>,
    pub versions: Vec<crate::config::ToolVersion>,
    pub install: crate::remote::ToolInstall,
    pub changed: Vec<(String, u64)>,
    pub warnings: Vec<String>,
    pub permission_mode: Option<String>,
    pub continue_with: Option<String>,
}

/// Where the target of a plan comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum TargetSource {
    Flag,
    Project,
    Personal(PathBuf),
    Chosen(PathBuf),
}

impl TargetSource {
    /// The text after the target on the destination line. A target from --to has none.
    pub fn label(&self) -> Option<String> {
        match self {
            Self::Flag => None,
            Self::Project => Some("project default, beam.toml [beam] to".into()),
            Self::Personal(p) => Some(format!("personal default, {}", p.display())),
            Self::Chosen(p) => Some(format!("saved as personal default, {}", p.display())),
        }
    }

    /// How to use a different target when the configured one fails its checks.
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            Self::Flag => None,
            Self::Project => {
                Some("Use `beam --to TARGET` for this run, or change [beam] to in beam.toml")
            }
            Self::Personal(_) | Self::Chosen(_) => Some(
                "Use `beam --to TARGET` for this run, or `beam default TARGET` to change your personal default",
            ),
        }
    }
}

/// The configured target in precedence order: --to, project [beam] to, then personal to.
fn configured_target(
    flag: Option<String>,
    project: Option<String>,
    personal: Option<String>,
    personal_path: &Path,
) -> Option<(String, TargetSource)> {
    flag.map(|t| (t, TargetSource::Flag))
        .or_else(|| project.map(|t| (t, TargetSource::Project)))
        .or_else(|| personal.map(|t| (t, TargetSource::Personal(personal_path.into()))))
}

pub fn ask(prompt: &str) -> Result<String> {
    if !std::io::stdin().is_terminal() {
        bail!("{prompt} Supply the option explicitly when stdin is not a terminal");
    }
    print!("{} ", ui::question(prompt));
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

/// A destination choice: target, description, and optional readiness with its reason.
type Choice<'a> = (&'a str, &'a str, Option<(bool, &'a str)>);

/// The menu index for an answer: Enter selects the default, a number selects that line, and other text is None.
fn menu_pick(answer: &str, default: usize, len: usize) -> Result<Option<usize>> {
    if answer.is_empty() {
        return Ok(Some(default));
    }
    match answer.parse::<usize>() {
        Ok(n) if (1..=len).contains(&n) => Ok(Some(n - 1)),
        Ok(_) => bail!("destination number is out of range"),
        Err(_) => Ok(None),
    }
}

/// Ask for a destination from a short menu that shows which providers look ready on this machine.
fn choose_target() -> Result<String> {
    let docker = util::succeeds(std::process::Command::new("docker").args([
        "info",
        "--format",
        "{{.ServerVersion}}",
    ]));
    let steel = util::succeeds(std::process::Command::new("steel").arg("--version"));
    let daytona = std::env::var_os("DAYTONA_API_KEY").is_some_and(|v| !v.is_empty());
    let choices: [Choice; 5] = [
        (
            "docker",
            "Docker on this machine",
            Some((
                docker,
                if docker {
                    "Docker is running"
                } else {
                    "Docker is not running"
                },
            )),
        ),
        ("docker+ssh://HOST", "Docker on a remote host", None),
        ("ssh://HOST", "a Linux host over SSH", None),
        (
            "steel",
            "Steel cloud computer",
            Some((
                steel,
                if steel {
                    "steel CLI found"
                } else {
                    "steel CLI not found"
                },
            )),
        ),
        (
            "daytona",
            "Daytona cloud sandbox",
            Some((
                daytona,
                if daytona {
                    "DAYTONA_API_KEY is set"
                } else {
                    "DAYTONA_API_KEY is not set"
                },
            )),
        ),
    ];
    // The default is the first local or cloud destination that looks ready.
    let default = [(0, docker), (3, steel), (4, daytona)]
        .iter()
        .find(|(_, ready)| *ready)
        .map_or(0, |(i, _)| *i);
    ui::heading("Choose a destination for this workspace.");
    println!();
    for (i, (name, about, status)) in choices.iter().enumerate() {
        let status = match status {
            Some((true, text)) => format!("{} {text}", ui::paint(Hue::Green, "●")),
            Some((false, text)) => ui::dim(&format!("○ {text}")),
            None => String::new(),
        };
        let mark = if i == default {
            ui::bold(Hue::Green, "  ← default")
        } else {
            String::new()
        };
        println!(
            "  {} {} {}  {status}{mark}",
            ui::bold(Hue::Turquoise, &format!("{}.", i + 1)),
            ui::bold(Hue::Blue, &format!("{name:<18}")),
            format_args!("{about:<24}"),
        );
    }
    println!(
        "\n{}\n",
        ui::dim("Enter a number, or a full target such as steel:CHECKPOINT or daytona:SNAPSHOT.")
    );
    let answer = ask(&format!(
        "Destination [1-{}; default: {}]:",
        choices.len(),
        default + 1
    ))?;
    let Some(i) = menu_pick(&answer, default, choices.len())? else {
        return Ok(answer);
    };
    let (name, _, _) = choices[i];
    if let Some(prefix) = name.strip_suffix("HOST") {
        let host = ask("SSH host [user@host or a Host from ~/.ssh/config]:")?;
        if host.is_empty() {
            bail!("an SSH host is required for {name}");
        }
        return Ok(format!("{prefix}{host}"));
    }
    Ok(name.to_string())
}

fn session_age(seconds: u64) -> String {
    let (count, unit) = match seconds {
        0..60 => return "just now".into(),
        60..3600 => (seconds / 60, "minute"),
        3600..172800 => (seconds / 3600, "hour"),
        _ => (seconds / 86400, "day"),
    };
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {unit}{plural} ago")
}

/// Expand files without following links in extras. User configuration may link within HOME.
pub fn files(base: &Path, rel: &str) -> Result<Vec<String>> {
    files_inner(
        base,
        rel,
        false,
        &mut std::collections::BTreeSet::new(),
        &mut vec![],
    )
}
/// A link back into a directory that is already in the walk adds no files, so it gives a warning.
fn user_files(base: &Path, rel: &str, warnings: &mut Vec<String>) -> Result<Vec<String>> {
    files_inner(
        base,
        rel,
        true,
        &mut std::collections::BTreeSet::new(),
        warnings,
    )
}
fn files_inner(
    base: &Path,
    rel: &str,
    follow: bool,
    parents: &mut std::collections::BTreeSet<PathBuf>,
    warnings: &mut Vec<String>,
) -> Result<Vec<String>> {
    util::relative_path(rel)?;
    let path = base.join(rel);
    if !path.exists() && !path.is_symlink() {
        return Ok(vec![]);
    }
    let canonical = path
        .canonicalize()
        .with_context(|| format!("cannot read {}", path.display()))?;
    if !canonical.starts_with(base.canonicalize()?) {
        bail!(
            "{} points outside {}. Copy it inside first",
            path.display(),
            base.display()
        );
    }
    if path.is_symlink() && !follow {
        bail!(
            "extra {} is a symlink. Use a regular file or directory",
            path.display()
        );
    }
    let meta = std::fs::metadata(&path)?;
    if meta.is_file() {
        return Ok(vec![rel.into()]);
    }
    if !meta.is_dir() {
        bail!("unsupported file type: {}", path.display());
    }
    if !parents.insert(canonical.clone()) {
        warnings.push(format!("skipping symlink cycle: {}", path.display()));
        return Ok(vec![]);
    }
    let mut out = vec![];
    for e in std::fs::read_dir(path)? {
        let name = e?
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("file names must be UTF-8"))?;
        out.extend(files_inner(
            base,
            &format!("{rel}/{name}"),
            follow,
            parents,
            warnings,
        )?);
    }
    parents.remove(&canonical);
    out.sort();
    Ok(out)
}

pub fn hashes(base: &Path, names: &[String]) -> Result<BTreeMap<String, String>> {
    names
        .iter()
        .map(|n| Ok((n.clone(), util::sha256_file(&base.join(n))?)))
        .collect()
}

/// Node.js major version when neither the project nor this machine has one: the current LTS line.
const DEFAULT_NODE: &str = "22";

/// What a sandbox that Beam creates installs when a known tool is missing.
/// A project pin wins. Then the version on this machine, so the sandbox matches local work.
fn tool_install(
    target: &str,
    tools: &[String],
    versions: &[crate::config::ToolVersion],
    local: impl Fn(&str) -> Option<String>,
) -> crate::remote::ToolInstall {
    let pin = |tool: &str| {
        versions
            .iter()
            .find(|p| p.tool == tool)
            .map(|p| p.version.clone())
    };
    let needs = |names: &[&str]| tools.iter().any(|t| names.contains(&t.as_str()));
    let node = if needs(&["node", "npm", "npx", "pnpm"]) {
        pin("node")
            .or_else(|| local("node").map(|v| v.split('.').next().unwrap_or("").to_string()))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| DEFAULT_NODE.into())
    } else {
        String::new()
    };
    let pnpm = if needs(&["pnpm"]) {
        pin("pnpm")
            .or_else(|| local("pnpm"))
            .unwrap_or_else(|| "latest".into())
    } else {
        String::new()
    };
    crate::remote::ToolInstall {
        enabled: !matches!(
            crate::sandbox::Target::parse(target),
            Ok(crate::sandbox::Target::Ssh { .. })
        ),
        node,
        pnpm,
        uv: if needs(&["uv"]) {
            pin("uv").unwrap_or_else(|| "latest".into())
        } else {
            String::new()
        },
        rust: if needs(&["cargo", "rustc"]) {
            pin("rustc").unwrap_or_else(|| "stable".into())
        } else {
            String::new()
        },
    }
}

/// The numeric version that `tool --version` prints on this machine.
fn local_version(tool: &str) -> Option<String> {
    let output = std::process::Command::new(tool)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let version: String = text
        .trim()
        .trim_start_matches('v')
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    (output.status.success() && version.contains('.')).then_some(version)
}

impl Plan {
    /// Show a failed destination check with a hint when the target is remembered, not given with --to.
    pub fn target_failure(&self, e: anyhow::Error) -> anyhow::Error {
        let Some(hint) = self.target_source.hint() else {
            return e;
        };
        eprintln!("{}", ui::error(&format!("{e:#}")));
        eprintln!("{}", ui::dim(hint));
        ui::Reported.into()
    }

    pub fn preflight_tools(&self) -> Vec<String> {
        if self.agent.capabilities().environment_repair {
            agent::transfer_tools(self.agent)
        } else {
            self.tools.clone()
        }
    }

    pub fn preflight_versions(&self) -> &[crate::config::ToolVersion] {
        if self.agent.capabilities().environment_repair {
            &[]
        } else {
            &self.versions
        }
    }

    pub fn build(a: &crate::up::UpArgs) -> Result<Self> {
        let home = crate::up::home_dir()?;
        let cwd = a
            .path
            .canonicalize()
            .with_context(|| format!("{} does not exist", a.path.display()))?;
        let root = git::toplevel(&cwd)?;
        git::ensure_has_commits(&root)?;
        let mut config = Config::load(&root)?;
        config.workflow.pr |= a.pr;
        config.workflow.github_auth |= a.github_auth || config.workflow.pr;
        if config.workflow.github_auth {
            crate::publication::github_origin(&git::config_get(&root, "remote.origin.url"))?;
        }
        let user = UserConfig::load(&home)?;
        let user_path = UserConfig::path(&home);
        let (target, target_source) = if let Some(found) =
            configured_target(a.to.clone(), config.beam.to.clone(), user.to, &user_path)
        {
            found
        } else {
            if a.yes {
                bail!(
                    "no target. Supply the option explicitly with --to TARGET, or configure a personal default"
                );
            }
            let t = choose_target()?;
            Target::parse(&t)?;
            if !a.dry_run {
                UserConfig {
                    to: Some(t.clone()),
                }
                .save(&home)?;
            }
            (t, TargetSource::Chosen(user_path))
        };
        let parsed = Target::parse(&target)?;
        // Fail before the session choice and the plan.
        if a.build_image && !matches!(parsed, Target::Docker { .. }) {
            bail!(
                "--build-image needs a Docker target, and {target} is not one. To build the image without a transfer: beam build-image"
            );
        }
        let mut adapter = if a.agent == "auto" {
            agent::default_adapter()
        } else {
            agent::get(&a.agent)?
        };
        if !adapter.capabilities().session_transfer && a.session.is_some() {
            bail!("--session cannot be used with --agent {}", adapter.id());
        }
        // Check before the session menu, so that a busy project does not ask for a choice.
        if !a.force && adapter.is_running(&root) {
            bail!(
                "{} is still running in this project. Exit it first, or use --force",
                adapter.label()
            );
        }
        let mut warnings = vec![];
        let session = if !adapter.capabilities().session_transfer {
            None
        } else if let Some(id) = &a.session {
            Some(adapter.find_session(&home, &cwd, id)?)
        } else {
            let mut sessions = adapter.sessions(&home, &cwd)?;
            if sessions.is_empty() && cwd != root {
                sessions = adapter.sessions(&home, &root)?;
            }
            if sessions.is_empty() {
                if a.agent != "auto" {
                    bail!(
                        "no {} session found. Start the agent locally, or use --agent shell",
                        adapter.label()
                    );
                }
                None
            } else if sessions.len() > 1 && !a.yes && std::io::stdin().is_terminal() {
                println!();
                ui::heading(format!(
                    "Found {} saved {} sessions for {}.",
                    sessions.len(),
                    adapter.label(),
                    ui::link(&root)
                ));
                println!("Choose a conversation to continue on {target}.");
                println!(
                    "{}\n",
                    ui::dim("Titles come from saved conversations; they are not commands to run.")
                );
                for (i, s) in sessions.iter().enumerate() {
                    let default = if i == 0 { " (default)" } else { "" };
                    // Debug quoting keeps control characters in titles from reaching the terminal.
                    println!(
                        "  {} {}{}",
                        ui::bold(Hue::Turquoise, &format!("{}.", i + 1)),
                        ui::strong(&format!("{:?}", s.title)),
                        ui::paint(Hue::Green, default)
                    );
                    println!(
                        "{}\n",
                        ui::dim(&format!(
                            "     Last updated: {} | Session ID: {}",
                            session_age(s.modified.elapsed().unwrap_or_default().as_secs()),
                            s.id
                        ))
                    );
                }
                println!(
                    "{}\n",
                    ui::dim("For workspace only, press Ctrl-C and rerun with --agent shell.")
                );
                let answer = ask(&format!(
                    "Choose a session [1-{}; default: 1]:",
                    sessions.len()
                ))?;
                let selected = if answer.is_empty() {
                    1
                } else {
                    answer.parse::<usize>().context("enter a session number")?
                };
                if selected == 0 || selected > sessions.len() {
                    bail!("session number is out of range");
                }
                Some(sessions.remove(selected - 1))
            } else {
                Some(sessions.remove(0))
            }
        };
        let cwd = session.as_ref().map(|s| s.cwd.clone()).unwrap_or(cwd);
        if a.agent == "auto" && session.is_none() {
            adapter = agent::get("shell")?;
        }
        let mut extras = config.extras();
        if config.files.extras.is_none() {
            extras.retain(|rel| {
                root.join(rel).exists()
                    && util::succeeds(git::git(&root).args(["check-ignore", "-q", "--", rel]))
            });
        }
        for rel in &config.files.return_extras {
            if !extras.contains(rel) {
                extras.push(rel.clone());
            }
        }
        let mut extra_files = vec![];
        for rel in &extras {
            util::relative_path(rel)?;
            if rel == ".git"
                || rel.starts_with(".git/")
                || rel == ".beam"
                || rel.starts_with(".beam/")
            {
                bail!("cannot transfer internal path {rel}");
            }
            let found = files(&root, rel)?;
            if found.is_empty() && !config.files.return_extras.contains(rel) {
                warnings.push(format!("extra {rel} is absent"));
            }
            extra_files.extend(found);
        }
        extra_files.sort();
        extra_files.dedup();
        for rel in &extra_files {
            if util::succeeds(git::git(&root).args(["ls-files", "--error-unmatch", "--", rel])) {
                bail!("extra {rel} is tracked by Git. Remove it from [files] extras/return_extras");
            }
            if !util::succeeds(git::git(&root).args(["check-ignore", "-q", "--", rel])) {
                bail!("extra {rel} must be ignored by Git");
            }
        }
        let mut agent_files = vec![];
        let mut defaults = vec![];
        if let Some(s) = &session {
            for rel in adapter.session_paths(&home, s) {
                agent_files.extend(user_files(&home, &rel, &mut warnings)?);
            }
            for rel in adapter.user_paths(&home) {
                defaults.extend(user_files(&home, &rel, &mut warnings)?);
            }
        }
        let changed = git::changed_files(&root)?;
        let max = util::parse_size(
            config
                .files
                .max_file_size
                .as_deref()
                .unwrap_or(DEFAULT_MAX_FILE_SIZE),
        )?;
        let mut large: Vec<(String, u64)> = vec![];
        // Validate every current Git file as well as index blobs, extras, and agent files.
        let output = git::git(&root)
            .args([
                "ls-files",
                "-z",
                "--cached",
                "--others",
                "--exclude-standard",
            ])
            .output()?;
        if !output.status.success() {
            bail!("cannot list Git files");
        }
        for rel in String::from_utf8(output.stdout)?
            .split('\0')
            .filter(|p| !p.is_empty())
        {
            if rel.starts_with(".beam/") {
                continue;
            }
            let p = root.join(rel);
            if p.file_name().is_some_and(|n| n == ".gitattributes")
                && std::fs::read_to_string(&p).is_ok_and(|s| s.contains("filter=lfs"))
            {
                bail!(
                    "Git LFS is configured in {rel}. Beam cannot transfer its separate object store yet"
                );
            }
            if let Ok(meta) = std::fs::symlink_metadata(&p) {
                if meta.file_type().is_symlink()
                    && !p.canonicalize().is_ok_and(|p| p.starts_with(&root))
                {
                    warnings.push(format!("{rel}: external symlink target is not transferred"));
                }
                if meta.len() > max {
                    large.push((rel.into(), meta.len()));
                }
            }
        }
        for (name, size) in git::indexed_sizes(&root)? {
            if size > max {
                large.push((format!("{name} (staged)"), size));
            }
        }
        for (base, names) in [
            (&root, &extra_files),
            (&home, &agent_files),
            (&home, &defaults),
        ] {
            for rel in names {
                let size = std::fs::metadata(base.join(rel))?.len();
                if size > max {
                    large.push((rel.clone(), size));
                }
            }
        }
        if !a.allow_large && !large.is_empty() {
            bail!(
                "these files are larger than {}:\n{}\nUse --allow-large to include them",
                util::human_size(max),
                large
                    .iter()
                    .map(|(n, s)| format!("  {n} ({})", util::human_size(*s)))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
        let mut names = config.env.forward.clone();
        if session.is_some() {
            names.extend(adapter.auth_env().iter().map(|s| s.to_string()));
        }
        names.sort();
        names.dedup();
        for name in &names {
            if name.is_empty()
                || !name.bytes().enumerate().all(|(i, c)| {
                    c == b'_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit())
                })
            {
                bail!("invalid environment variable name {name:?}");
            }
        }
        let mut env: Vec<_> = names
            .iter()
            .filter_map(|n| std::env::var(n).ok().map(|v| (n.clone(), v)))
            .collect();
        if session.is_some() {
            // A variable in [env] forward is sent even when the agent does not need it.
            let unused = adapter.unused_auth(&env);
            env.retain(|(k, _)| !unused.contains(&k.as_str()) || config.env.forward.contains(k));
        }
        if config.workflow.github_auth {
            let token = crate::publication::github_token()?;
            env.retain(|(name, _)| {
                !["GH_TOKEN", "GITHUB_TOKEN", "GH_PROMPT_DISABLED"].contains(&name.as_str())
            });
            env.push(("GH_TOKEN".into(), token));
            env.push(("GH_PROMPT_DISABLED".into(), "1".into()));
        }
        for n in &config.env.forward {
            if !(env.iter().any(|(k, _)| k == n)
                || (config.workflow.github_auth && n == "GITHUB_TOKEN"))
            {
                warnings.push(format!("{n} is not set locally"));
            }
        }
        if session.is_some() && !adapter.has_auth(&env) {
            warnings.push(format!(
                "{} authentication is not forwarded. Log in after `beam attach`",
                adapter.label()
            ));
        }
        for input in &config.sandbox.setup_inputs {
            util::relative_path(input)?;
        }
        let setup = config.setup(&root);
        let mut tools = agent::transfer_tools(adapter);
        if config.workflow.github_auth {
            tools.push("gh".into());
        }
        if config.sandbox.setup.is_none() {
            tools.extend(
                crate::config::detected_rules(&root)
                    .iter()
                    .map(|r| r.tool.into()),
            );
        }
        let (versions, version_warnings) = crate::config::tool_versions(&root)?;
        warnings.extend(version_warnings);
        for pin in &versions {
            if !tools.contains(&pin.tool) {
                tools.push(pin.tool.clone());
            }
        }
        let install = tool_install(&target, &tools, &versions, local_version);
        let return_extras = config.files.return_extras.clone();
        let permission_mode = match (&session, &a.permission_mode) {
            (None, Some(_)) => bail!("--permission-mode needs an agent session"),
            (None, None) => None,
            (Some(_), mode) => mode
                .clone()
                .or_else(|| config.agent.permission_mode.clone()),
        };
        if let Some(mode) = &permission_mode
            && (mode.is_empty() || !mode.bytes().all(|c| c.is_ascii_alphabetic()))
        {
            bail!("invalid permission mode {mode:?}");
        }
        let continue_with = a.continue_with.clone().filter(|t| !t.trim().is_empty());
        Ok(Self {
            root,
            cwd,
            home,
            config,
            target,
            target_source,
            session,
            agent: adapter,
            extras,
            return_extras,
            extra_files,
            agent_files,
            defaults,
            env,
            setup,
            tools,
            versions,
            install,
            changed,
            warnings,
            permission_mode,
            continue_with,
        })
    }

    pub fn show(&self, details: bool) {
        crate::up::step("project", self.root.display().to_string());
        crate::up::step(
            "destination",
            match self.target_source.label() {
                Some(from) => format!("{} {}", self.target, ui::dim(&format!("({from})"))),
                None => self.target.clone(),
            },
        );
        crate::up::step(
            "session",
            self.session
                .as_ref()
                .map(|s| format!("{} · {} · {} turns", self.agent.label(), s.title, s.turns))
                .unwrap_or_else(|| "shell workspace (no agent session)".into()),
        );
        if let Some(mode) = &self.permission_mode {
            crate::up::step("permissions", mode);
        }
        if let Some(text) = &self.continue_with {
            crate::up::step("continue", text);
        }
        println!(
            "\n{} complete reachable Git history, staged and unstaged changes, and untracked files.",
            ui::bold(Hue::Turquoise, "Send:")
        );
        if self.session.is_some() {
            println!(
                "{} {} session and configuration. {} session files.",
                ui::bold(Hue::Turquoise, "Send:"),
                self.agent.label(),
                ui::bold(Hue::Green, "Return:")
            );
        }
        println!(
            "{} remote Git work. Beam combines supported separate edits and saves conflicts for review.",
            ui::bold(Hue::Green, "Return:")
        );
        println!(
            "{} running processes, databases, and ignored build output.",
            ui::bold(Hue::Orange, "Stay local:")
        );
        for rel in &self.extras {
            crate::up::step(
                "extra",
                format!(
                    "{rel} — {}",
                    if self.return_extras.contains(rel) {
                        "send and return"
                    } else {
                        "send only; remote edits do not return"
                    }
                ),
            );
        }
        if self.config.workflow.github_auth {
            crate::up::step(
                "GitHub",
                "authentication forwarded for Git and gh; values are hidden",
            );
        }
        if self.config.workflow.pr {
            crate::up::step(
                "workflow",
                "task branch, Conventional Commits, push, and draft pull request",
            );
        }
        if let Some(text) = self.install.describe() {
            crate::up::step("tools", text);
        }
        if self.config.sandbox.cache.unwrap_or(true)
            && matches!(
                crate::sandbox::Target::parse(&self.target),
                Ok(crate::sandbox::Target::Docker { .. })
            )
        {
            crate::up::step(
                "cache",
                format!(
                    "package downloads stay in the {} Docker volume",
                    crate::sandbox::CACHE_VOLUME
                ),
            );
        }
        crate::up::step("worktree", format!("{} changed paths", self.changed.len()));
        if details && let Some(n) = git::unpushed_count(&self.root) {
            crate::up::step("commits", format!("{n} unpushed"));
        }
        crate::up::step(
            "env",
            if self.env.is_empty() {
                "none".into()
            } else if !details {
                format!("{} variables; values are hidden", self.env.len())
            } else {
                self.env
                    .iter()
                    .map(|(k, _)| k.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            },
        );
        crate::up::step(
            "setup",
            if self.setup.is_empty() {
                "none".into()
            } else if !details {
                format!("{} commands", self.setup.len())
            } else {
                self.setup.join("; ")
            },
        );
        crate::up::step(
            "verify",
            if self.config.sandbox.verify.is_empty() {
                "none; project readiness is unverified".into()
            } else if !details {
                format!("{} project checks", self.config.sandbox.verify.len())
            } else {
                self.config.sandbox.verify.join("; ")
            },
        );
        if self.agent.capabilities().environment_repair {
            println!(
                "{}",
                ui::dim(
                    "Project tools are checked after upload. The agent receives failed checks and repairs the environment before continuing."
                )
            );
        }
        for line in self.config.task.handoff().split_inclusive('\n') {
            match line.split_once(": ") {
                Some((label, value)) => print!("{} {value}", ui::dim(&format!("{label}:"))),
                None => print!("{line}"),
            }
        }
        if details && let Some(session) = &self.session {
            crate::up::step("session id", &session.id);
        }
        for pin in self.versions.iter().filter(|_| details) {
            crate::up::step("toolchain", format!("{} {}", pin.tool, pin.version));
        }
        for warning in &self.warnings {
            crate::ui::warn(warning);
        }
        if !details {
            println!(
                "Details: add --details, or run beam doctor --to {} --agent {}",
                util::sh_quote(&self.target),
                self.agent.id()
            );
        }
    }
}

#[cfg(test)]
mod destination_tests {
    use super::{TargetSource, configured_target, menu_pick};
    use std::path::Path;

    #[test]
    fn configured_target_follows_precedence_and_records_the_source() {
        let p = Path::new("/h/.config/beam/config.toml");
        let s = |v: &str| Some(v.to_string());
        assert_eq!(
            configured_target(s("docker"), s("steel"), s("daytona"), p),
            Some(("docker".into(), TargetSource::Flag))
        );
        assert_eq!(
            configured_target(None, s("steel"), s("daytona"), p),
            Some(("steel".into(), TargetSource::Project))
        );
        assert_eq!(
            configured_target(None, None, s("daytona"), p),
            Some(("daytona".into(), TargetSource::Personal(p.into())))
        );
        assert_eq!(configured_target(None, None, None, p), None);
    }

    #[test]
    fn remembered_targets_explain_how_to_change_them() {
        let p = Path::new("/h/.config/beam/config.toml");
        assert_eq!(TargetSource::Flag.label(), None);
        assert_eq!(TargetSource::Flag.hint(), None);
        assert!(TargetSource::Project.hint().unwrap().contains("beam.toml"));
        for source in [
            TargetSource::Personal(p.into()),
            TargetSource::Chosen(p.into()),
        ] {
            assert!(source.label().unwrap().contains("personal default"));
            assert!(source.hint().unwrap().contains("beam default TARGET"));
        }
    }

    #[test]
    fn menu_pick_reads_enter_numbers_and_targets() {
        assert_eq!(menu_pick("", 3, 5).unwrap(), Some(3));
        assert_eq!(menu_pick("1", 3, 5).unwrap(), Some(0));
        assert_eq!(menu_pick("5", 0, 5).unwrap(), Some(4));
        assert_eq!(menu_pick("steel:abc", 0, 5).unwrap(), None);
        assert!(menu_pick("0", 0, 5).is_err());
        assert!(menu_pick("6", 0, 5).is_err());
    }
}

#[cfg(test)]
mod user_files_tests {
    use super::user_files;

    #[test]
    fn user_files_skip_symlink_cycles_with_a_warning() {
        let home = tempfile::tempdir().unwrap();
        let skill = home.path().join(".claude/skills/x");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), "x").unwrap();
        std::os::unix::fs::symlink(&skill, skill.join("x")).unwrap();
        let mut warnings = vec![];
        let found = user_files(home.path(), ".claude/skills", &mut warnings).unwrap();
        assert_eq!(found, vec![".claude/skills/x/SKILL.md".to_string()]);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("skipping symlink cycle"));
        assert!(warnings[0].contains(".claude/skills/x/x"));
    }
}

#[cfg(test)]
mod tool_install_tests {
    use super::tool_install;
    use crate::config::ToolVersion;

    fn pin(tool: &str, version: &str) -> ToolVersion {
        ToolVersion {
            tool: tool.into(),
            version: version.into(),
        }
    }

    #[test]
    fn project_pins_win_then_local_versions_then_defaults() {
        let tools: Vec<String> = ["git", "pnpm", "cargo"].map(String::from).to_vec();
        let local = |tool: &str| match tool {
            "node" => Some("26.4.0".to_string()),
            "pnpm" => Some("11.11.0".to_string()),
            _ => None,
        };
        let from_local = tool_install("docker", &tools, &[], local);
        assert!(from_local.enabled);
        assert_eq!(from_local.node, "26");
        assert_eq!(from_local.pnpm, "11.11.0");
        assert_eq!(from_local.rust, "stable");
        assert_eq!(from_local.uv, "");
        let pinned = tool_install(
            "steel",
            &tools,
            &[
                pin("node", "22.14.0"),
                pin("pnpm", "10.2.0"),
                pin("rustc", "1.90"),
            ],
            local,
        );
        assert_eq!(
            (
                pinned.node.as_str(),
                pinned.pnpm.as_str(),
                pinned.rust.as_str()
            ),
            ("22.14.0", "10.2.0", "1.90")
        );
        let nothing_local = tool_install("daytona", &tools, &[], |_| None);
        assert_eq!(nothing_local.node, "22");
        assert_eq!(nothing_local.pnpm, "latest");
    }

    #[test]
    fn ssh_hosts_and_projects_without_known_tools_install_nothing() {
        let tools: Vec<String> = ["git", "pnpm"].map(String::from).to_vec();
        assert!(!tool_install("ssh://box", &tools, &[], |_| None).enabled);
        let plain = tool_install("docker", &["git".into()], &[], |_| None);
        assert_eq!(plain.describe(), None);
    }
}
