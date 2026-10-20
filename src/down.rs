// ABOUTME: Downloads once, saves recovery data, and applies the return without overwriting local work.
use crate::{
    agent,
    git::{self, BackOutcome, Snap},
    pack::{self, DiskEntry},
    presentation, remote,
    return_files::{MergeDecision, merge_decision},
    state::{Phase, ProjectLock, State},
    ui::{self, Hue},
    up::step,
    util,
};
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::Path;

pub fn load_state(path: &Path) -> Result<State> {
    let cwd = path.canonicalize()?;
    let root = git::toplevel(&cwd)?;
    State::load(&root)?.context("this project is local. Run `beam` to send it")
}

/// The remote pack. Local `beam down` and `beam down` in the sandbox use the same script.
pub fn pack_script(st: &State) -> Result<String> {
    Ok(remote::pack_back(
        &st.stage,
        &st.project_root.to_string_lossy(),
        &st.remote_home,
        &format!("refs/beam/{}/back", st.transfer_id),
        &st.sent.wt_commit,
        &agent::get(&st.agent)?.return_paths(&st.agent_cwd),
        &st.return_extras,
    ))
}

/// Wait until `beam down` in the sandbox has packed the work or has failed. Hold no lock while waiting.
fn wait_for_sandbox(path: &Path) -> Result<()> {
    let st = load_state(path)?;
    if !matches!(st.phase, Phase::Starting | Phase::Remote) {
        return Ok(());
    }
    let sb = st.sandbox()?;
    let script = remote::return_progress(&st.stage);
    ui::say("Waiting for `beam down` in the sandbox. Stop with Ctrl-C.");
    let mut shown = String::new();
    let mut failures = 0;
    loop {
        match sb.exec_when_running(&script) {
            Ok(progress) => {
                failures = 0;
                if progress == "return-ready" {
                    return Ok(());
                }
                if let Some(reason) = progress.strip_prefix("return-failed") {
                    ui::warn(format!(
                        "The sandbox could not pack the work:{reason}. Beam tries again."
                    ));
                    return Ok(());
                }
                if progress != shown {
                    let note = match progress.as_str() {
                        "return-requested" => "The sandbox is packing the work.",
                        "none" => "No return was requested in the sandbox yet.",
                        _ => "The sandbox is not running. Beam waits until it runs.",
                    };
                    println!("{}", ui::dim(note));
                    shown = progress;
                }
            }
            Err(e) => {
                failures += 1;
                if failures >= 10 {
                    return Err(e.context("cannot check the sandbox"));
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(5));
    }
}

pub fn down(path: &Path, keep: bool, review: bool, wait: bool, detach: bool) -> Result<()> {
    crate::transporter::direction(true);
    if wait {
        wait_for_sandbox(path)?;
    }
    let root = git::toplevel(&path.canonicalize()?)?;
    let lock = ProjectLock::acquire(&crate::up::home_dir()?, &root)?;
    let mut st = load_state(path)?;
    let mut busy = ui::Busy::start();
    let result = return_home(&mut st, keep, review, detach);
    if let Err(e) = &result {
        busy.fail();
        st.last_error = Some(format!("{e:#}"));
        let _ = st.save();
        if st.phase != Phase::Closed && st.phase != Phase::Retained {
            if !st.conflicts.is_empty() {
                presentation::recovery(&st);
            }
            presentation::show(&st, None);
        }
    }
    let Some(command) = result? else {
        return Ok(());
    };
    // The agent replaces this process, so the lock and the tab progress must end first.
    drop(busy);
    drop(lock);
    use std::os::unix::process::CommandExt;
    let error = std::process::Command::new("sh")
        .args(["-c", &command])
        .current_dir(&st.agent_cwd)
        .exec();
    Err(error).with_context(|| format!("cannot start the agent: {command}"))
}

/// Beam starts the agent itself only after a clean return in a terminal, and only when the
/// agent does not already run in the project. Otherwise it prints the command.
fn starts_agent(conflicts: bool, detach: bool, terminal: bool, running: bool) -> bool {
    !conflicts && !detach && terminal && !running
}

/// Returns the agent command to start, when beam starts it.
fn return_home(st: &mut State, keep: bool, review: bool, detach: bool) -> Result<Option<String>> {
    if st.phase == Phase::Retained {
        if keep {
            presentation::show(st, None);
            return Ok(None);
        }
        ui::task(
            "cleanup",
            "removing retained sandbox; later sandbox edits will not return…",
            || crate::up::cleanup(st),
        )?;
        st.remove()?;
        ui::success("Removed the retained sandbox. Previously returned work is unchanged.");
        return Ok(None);
    }
    if matches!(
        st.phase,
        Phase::Planned | Phase::Allocating | Phase::Created | Phase::Prepared | Phase::Uploaded
    ) {
        bail!(
            "the upload is incomplete. Run `beam` to continue, or `beam kill --yes` to remove its resources"
        );
    }
    let package = st.dir().join("back.tar.gz");
    if !matches!(
        st.phase,
        Phase::Returning | Phase::Downloaded | Phase::Applied
    ) {
        st.advance(Phase::Returning)?;
    }
    // The workspace forms at home while the returned work is saved, then the animation stops.
    let mut show = None;
    if st.phase == Phase::Returning {
        let sb = st.sandbox()?;
        sb.wake()?;
        show = crate::show::Show::start(true);
        ui::task("agent", "stopping the remote session", || {
            let graceful = agent::get(&st.agent)?.graceful_stop();
            sb.exec(&remote::stop_launched_agent(&st.stage, &st.tmux, graceful))?;
            // The repair terminal is always an interactive shell.
            sb.exec(&remote::stop_agent(&format!("{}-repair", st.tmux), false))
        })?;
        let pack = pack_script(st)?;
        ui::task("pack", "packing remote work…", || sb.exec(&pack))?;
        let started = std::time::Instant::now();
        if let Some(show) = &show {
            show.toward(0.6);
        }
        ui::task("download", "downloading remote work…", || {
            sb.download(&remote::cat(&format!("{}/back.tar.gz", st.stage)), &package)
        })?;
        step(
            "download",
            util::size_and_rate(std::fs::metadata(&package)?.len(), started.elapsed()),
        );
        if let Some(show) = &show {
            show.arrive();
        }
        if let Ok(events) = sb.exec(&format!(
            "tail -n 100 {} 2>/dev/null || true",
            util::sh_quote(&format!("{}/events.tsv", st.stage))
        )) {
            util::atomic_write(&st.dir().join("events.tsv"), events.as_bytes())?;
        }
        st.advance(Phase::Downloaded)?;
    }
    if st.phase == Phase::Downloaded {
        step(
            "review",
            "saving remote work and preparing the return plan…",
        );
        let entries = pack::extract(
            &package,
            &st.dir().join("incoming"),
            &agent::get(&st.agent)?.return_paths(&st.agent_cwd),
        )?;
        if let Some(report) = entries.iter().find(|entry| entry.path == "publication.txt") {
            let text = std::fs::read_to_string(&report.file)?;
            util::atomic_write(&st.dir().join("publication.txt"), text.as_bytes())?;
            crate::publication::show_report(&text);
        }
        let info = entries
            .iter()
            .find(|e| e.path == "info")
            .context("return package has no Git snapshot")?;
        let snap = Snap::from_kv(&std::fs::read_to_string(&info.file)?)?;
        let bundle = entries
            .iter()
            .find(|e| e.path == "repo.bundle")
            .context("return package has no Git bundle")?;
        git::fetch_bundle(
            &st.project_root,
            &bundle.file,
            &format!("refs/beam/{}/back", st.transfer_id),
        )?;
        // This worktree also protects against interruption midway through applying the main worktree.
        let recovery = st.dir().join("worktree");
        if !st.dir().join("recovery.ready").exists() {
            git::recovery_worktree(&st.project_root, &recovery, &snap)?;
            util::atomic_write(&st.dir().join("recovery.ready"), b"ready")?;
        }
        st.recovery = Some(recovery.clone());
        st.save()?;
        drop(show.take());
        let existing = crate::review::load(st)?;
        let plan = if let Some(plan) = existing {
            let current = git::snapshot(
                &st.project_root,
                &format!("refs/beam/{}/review-check", st.transfer_id),
                None,
                None,
            )?;
            if !current.same_state(&plan.local)
                && !plan.target.as_ref().is_some_and(|t| current.same_state(t))
            {
                if st.dir().join("apply-started").exists() {
                    bail!(
                        "local work changed during an interrupted apply. Saved local and remote copies are in {}",
                        st.dir().display()
                    );
                }
                let updated = crate::review::build(st, &snap)?;
                crate::review::save(st, &updated)?;
                crate::review::show(st, &updated);
                bail!(
                    "local work changed after review. The plan was rebuilt; inspect it with beam review"
                );
            }
            plan
        } else {
            let plan = crate::review::build(st, &snap)?;
            crate::review::save(st, &plan)?;
            plan
        };
        let files = crate::return_files::prepare(st, &entries)?;
        crate::return_files::check(&files, st.dir().join("apply-started").exists())?;
        if review {
            let count = files.iter().filter(|f| f.conflict).count();
            let text = format!("{count} extra or agent files need review");
            println!(
                "{}",
                if count == 0 {
                    ui::dim(&text)
                } else {
                    ui::bold(Hue::Orange, &text)
                }
            );
            crate::review::show(st, &plan);
            println!(
                "{}",
                ui::dim("Local project files are unchanged. The remote session is stopped.")
            );
            return Ok(None);
        }
        util::atomic_write(&st.dir().join("apply-started"), b"started")?;
        let outcome = if let Some(target) = &plan.target {
            crate::review::prepare_undo(st, &plan, target)?;
            git::apply_back(
                &st.project_root,
                &plan.local,
                target,
                &format!("refs/beam/{}/check", st.transfer_id),
            )?
        } else {
            crate::review::prepare_undo(st, &plan, &plan.local)?;
            BackOutcome::KeptAside
        };
        if outcome == BackOutcome::KeptAside {
            let message = format!(
                "local Git state changed; remote work is in {}",
                recovery.display()
            );
            if !st.conflicts.contains(&message) {
                st.conflicts.push(message);
            }
        } else if plan
            .target
            .as_ref()
            .is_some_and(|t| t.same_state(&plan.local))
        {
            step("worktree", "no remote Git changes");
        } else {
            step("worktree", "remote changes applied");
        }
        let scopes = agent::get(&st.agent)?.return_paths(&st.agent_cwd);
        let agent: Vec<_> = entries
            .iter()
            .filter(|e| agent::contains_path(&scopes, &e.path))
            .collect();
        let report = merge_files(
            Some(agent::get(&st.agent)?),
            &st.home,
            &st.sent_files,
            &agent,
            "",
            &[],
            &st.dir().join("conflicts/agent"),
        )?;
        for rel in &report.replaced {
            step(
                "session",
                format!("remote conversation applied to {rel}; beam undo restores the local copy"),
            );
        }
        st.conflicts.extend(report.conflicts);
        let extras: Vec<_> = entries
            .iter()
            .filter(|e| e.path.starts_with("extras/"))
            .collect();
        for e in &extras {
            let rel = e.path.strip_prefix("extras/").unwrap();
            if !st
                .return_extras
                .iter()
                .any(|r| rel == r || rel.starts_with(&format!("{r}/")))
            {
                bail!("unrequested returning extra: {rel}");
            }
        }
        let report = merge_files(
            None,
            &st.project_root,
            &st.sent_extras,
            &extras,
            "extras/",
            &st.return_extras,
            &st.dir().join("conflicts/extras"),
        )?;
        st.conflicts.extend(report.conflicts);
        st.conflicts.sort();
        st.conflicts.dedup();
        st.advance(Phase::Applied)?;
    }
    // Applied is durable before cleanup. Cleanup failure never repeats the local apply.
    if st.phase == Phase::Applied {
        if keep {
            st.advance(Phase::Retained)?;
        } else {
            println!("{}", ui::dim("Return data is saved locally."));
            ui::task("cleanup", "removing sandbox…", || crate::up::cleanup(st))?;
            st.remove()?;
        }
    }
    if st.conflicts.is_empty() {
        ui::success(returned_message(
            trip_counts(st),
            &ui::link(&st.project_root),
        ));
    } else {
        ui::warn("Return finished with saved recovery. Local changes were preserved.");
        presentation::recovery(st);
    }
    if keep {
        println!(
            "Sandbox kept for inspection: {}.",
            ui::paint(Hue::Blue, &st.describe())
        );
        println!("{}", ui::paint(Hue::Orange, presentation::RETAINED_NOTICE));
    } else {
        println!("{}", ui::dim("Sandbox removed."));
    }
    println!("{}", ui::field("Recovery receipt", ui::link(&st.dir())));
    if let Some(line) = round_trip(st) {
        println!("{line}");
    }
    if let Some(note) = crate::monitor::repair_note(&crate::monitor::events(st)) {
        println!("{}", ui::dim(note));
    }
    let adapter = agent::get(&st.agent)?;
    let note = crate::handoff::return_note(&st.describe(), keep);
    let resume = adapter.resume_command(&st.session_id, Some(&note));
    if !st.conflicts.is_empty() {
        ui::next(presentation::recovery_action(st));
    } else if let Some(command) = &resume {
        let terminal = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
        if starts_agent(
            false,
            detach,
            terminal,
            adapter.is_running(&st.project_root),
        ) {
            ui::flavor(true);
            println!(
                "{}",
                ui::dim(&format!("Resuming the {} session.", adapter.label()))
            );
            return Ok(Some(command.clone()));
        }
        ui::next(command);
    } else {
        ui::next(format!(
            "cd {}",
            util::sh_quote(&st.project_root.to_string_lossy())
        ));
    }
    if !st.conflicts.is_empty() {
        bail!("return finished with conflicts; see the recovery paths above");
    }
    ui::flavor(true);
    Ok(None)
}

/// The result line after a return without conflicts. `counts` are the commits and paths from `trip_counts`.
fn returned_message(counts: Option<(usize, usize)>, root: &str) -> String {
    if counts == Some((0, 0)) {
        format!("No remote Git changes for {root}.")
    } else {
        format!("Remote work applied to {root}.")
    }
}

/// The commits and the changed paths that came home.
fn trip_counts(st: &State) -> Option<(usize, usize)> {
    let back = format!("refs/beam/{}/back", st.transfer_id);
    let count = |args: &[&str]| {
        util::run(git::git(&st.project_root).args(args))
            .ok()
            .map(|out| out.lines().filter(|l| !l.is_empty()).count())
    };
    let commits = util::run(git::git(&st.project_root).args([
        "rev-list",
        "--count",
        &format!("{}..{back}^1", st.sent.head),
    ]))
    .ok()?
    .parse::<usize>()
    .ok()?;
    let paths = count(&["diff", "--name-only", &st.sent.wt_commit, &back])?;
    Some((commits, paths))
}

/// One summary line for the trip: commits and paths that came home, time away, and conflicts.
fn round_trip(st: &State) -> Option<String> {
    let (commits, paths) = trip_counts(st)?;
    let away = util::now_unix().saturating_sub(st.created_at);
    let away = match away {
        s if s >= 3600 => format!("{}h {}m", s / 3600, s % 3600 / 60),
        s if s >= 60 => format!("{}m", s / 60),
        s => format!("{s}s"),
    };
    let plural =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let dot = ui::dim(" · ");
    let conflicts = st.conflicts.len();
    Some(format!(
        "{} {}{dot}{}{dot}{}{dot}{}{dot}{}",
        ui::bold(Hue::Purple, "◆"),
        ui::bold(Hue::Purple, "Home again"),
        ui::paint(Hue::Yellow, &plural(commits, "commit", "commits")),
        ui::paint(Hue::Yellow, &plural(paths, "path", "paths")),
        ui::paint(Hue::PaleYellow, &format!("{away} away")),
        ui::paint(
            if conflicts == 0 {
                Hue::Green
            } else {
                Hue::Orange
            },
            &plural(conflicts, "conflict", "conflicts")
        ),
    ))
}

/// Three-way merge for regular files, including local and remote deletions.
/// With an adapter, agent files where only the local agent's own turn was appended take the
/// remote copy (see `return_files::replaces_local_tail`).
pub fn merge_files(
    adapter: Option<&dyn agent::Adapter>,
    base: &Path,
    sent: &BTreeMap<String, String>,
    entries: &[&DiskEntry],
    prefix: &str,
    delete_scopes: &[String],
    conflicts: &Path,
) -> Result<MergeReport> {
    use std::os::unix::fs::PermissionsExt;
    let mut report = MergeReport::default();
    let mut returned = std::collections::BTreeSet::new();
    for e in entries {
        let rel = e
            .path
            .strip_prefix(prefix)
            .context("unexpected file prefix")?;
        returned.insert(rel.to_string());
        let dest = util::safe_destination(base, rel)?;
        let remote_hash = util::sha256_file(&e.file)?;
        let local_hash = if dest.is_file() {
            Some(util::sha256_file(&dest)?)
        } else {
            None
        };
        let mut decision = merge_decision(
            sent.get(rel).map(String::as_str),
            local_hash.as_deref(),
            Some(&remote_hash),
        );
        if decision == MergeDecision::Conflict
            && local_hash.is_some()
            && let Some(adapter) = adapter
            && crate::return_files::replaces_local_tail(
                adapter,
                rel,
                sent.get(rel).map(String::as_str),
                &dest,
                &e.file,
            )?
        {
            decision = MergeDecision::UseRemote;
            report.replaced.push(rel.to_string());
        }
        let target = match decision {
            MergeDecision::KeepLocal => continue,
            MergeDecision::UseRemote if !dest.is_dir() => dest,
            _ => {
                let target = util::safe_destination(conflicts, rel)?;
                report.conflicts.push(format!(
                    "{rel} changed on both sides; remote copy: {}",
                    target.display()
                ));
                target
            }
        };
        let parent = target.parent().context("file has no parent")?;
        std::fs::create_dir_all(parent)?;
        let tmp = tempfile::NamedTempFile::new_in(parent)?;
        std::fs::copy(&e.file, tmp.path())?;
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(e.mode))?;
        if e.mtime > 0 {
            tmp.as_file()
                .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(e.mtime))?;
        }
        tmp.as_file().sync_all()?;
        tmp.persist(&target).map_err(|e| e.error)?;
    }
    for (rel, hash) in sent {
        if returned.contains(rel)
            || !delete_scopes
                .iter()
                .any(|scope| rel == scope || rel.starts_with(&format!("{scope}/")))
        {
            continue;
        }
        let dest = util::safe_destination(base, rel)?;
        let local_hash = if dest.is_file() {
            Some(util::sha256_file(&dest)?)
        } else if !dest.exists() {
            None
        } else {
            report.conflicts.push(format!(
                "{rel} was deleted remotely and changed locally; local copy preserved"
            ));
            continue;
        };
        match merge_decision(Some(hash), local_hash.as_deref(), None) {
            MergeDecision::KeepLocal => {}
            MergeDecision::UseRemote => std::fs::remove_file(dest)?,
            MergeDecision::Conflict => report.conflicts.push(format!(
                "{rel} was deleted remotely and changed locally; local copy preserved"
            )),
        }
    }
    Ok(report)
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct MergeReport {
    pub conflicts: Vec<String>,
    /// Agent files where the remote copy replaced a local copy that only had the agent's own turn.
    pub replaced: Vec<String>,
}

#[cfg(test)]
mod message_tests {
    use super::returned_message;

    #[test]
    fn a_return_without_git_changes_does_not_claim_applied_work() {
        assert_eq!(
            returned_message(Some((0, 0)), "/p"),
            "No remote Git changes for /p."
        );
        assert_eq!(
            returned_message(Some((0, 2)), "/p"),
            "Remote work applied to /p."
        );
        assert_eq!(returned_message(None, "/p"), "Remote work applied to /p.");
    }
}

#[cfg(test)]
mod start_tests {
    use super::starts_agent;

    #[test]
    fn agent_starts_only_after_a_clean_return_in_a_free_terminal() {
        assert!(starts_agent(false, false, true, false));
        for (conflicts, detach, terminal, running) in [
            (true, false, true, false),
            (false, true, true, false),
            (false, false, false, false),
            (false, false, true, true),
        ] {
            assert!(!starts_agent(conflicts, detach, terminal, running));
        }
    }
}

#[cfg(test)]
mod merge_file_tests {
    use super::*;
    fn incoming(dir: &Path, name: &str, content: &str) -> DiskEntry {
        let file = dir.join(name);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, content).unwrap();
        DiskEntry {
            path: format!("extras/{name}"),
            file,
            mtime: 0,
            mode: 0o600,
        }
    }
    #[test]
    fn remote_session_replaces_a_local_copy_with_only_the_agents_own_turn() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("home");
        let rel = ".claude/projects/-w/s1.jsonl";
        let sent_text = "{\"type\":\"user\",\"message\":{\"content\":\"beam up\"}}\n";
        let own_turn = "{\"type\":\"assistant\",\"message\":{\"content\":[]}}\n";
        let remote_text = format!("{sent_text}{{\"type\":\"assistant\",\"remote\":1}}\n");
        let sent = BTreeMap::from([(rel.to_string(), util::sha256_bytes(sent_text.as_bytes()))]);
        let claude = agent::get("claude").unwrap();
        for (local, replaced) in [
            (format!("{sent_text}{own_turn}"), true),
            (
                format!(
                    "{sent_text}{own_turn}{{\"type\":\"user\",\"message\":{{\"content\":\"more\"}}}}\n"
                ),
                false,
            ),
            (format!("edited\n{own_turn}"), false),
        ] {
            std::fs::create_dir_all(home.join(".claude/projects/-w")).unwrap();
            std::fs::write(home.join(rel), &local).unwrap();
            let mut entry = incoming(&d.path().join("incoming"), rel, &remote_text);
            entry.path = rel.into();
            let report = merge_files(
                Some(claude),
                &home,
                &sent,
                &[&entry],
                "",
                &[],
                &d.path().join("conflicts"),
            )
            .unwrap();
            let now = std::fs::read_to_string(home.join(rel)).unwrap();
            if replaced {
                assert_eq!(report.replaced, vec![rel.to_string()]);
                assert!(report.conflicts.is_empty());
                assert_eq!(now, remote_text);
            } else {
                assert!(report.replaced.is_empty(), "{local}");
                assert_eq!(report.conflicts.len(), 1, "{local}");
                assert_eq!(now, local);
            }
        }
    }
    #[test]
    fn merge_handles_both_deletions_conflicts_and_retries() {
        let d = tempfile::tempdir().unwrap();
        let base = d.path().join("local");
        let input = d.path().join("incoming");
        std::fs::create_dir(&base).unwrap();
        let mut sent = BTreeMap::new();
        for name in [
            "changed",
            "deleted-local",
            "deleted-remote",
            "conflict",
            "local-only",
            "deleted-remotely-edited-locally",
        ] {
            std::fs::write(base.join(name), "old").unwrap();
            sent.insert(name.into(), util::sha256_bytes(b"old"));
        }
        std::fs::remove_file(base.join("deleted-local")).unwrap();
        std::fs::write(base.join("conflict"), "local").unwrap();
        std::fs::write(base.join("local-only"), "local").unwrap();
        std::fs::write(base.join("deleted-remotely-edited-locally"), "local").unwrap();
        let es = [
            incoming(&input, "changed", "remote"),
            incoming(&input, "deleted-local", "old"),
            incoming(&input, "conflict", "remote"),
            incoming(&input, "local-only", "old"),
        ];
        let refs: Vec<_> = es.iter().collect();
        let scopes: Vec<_> = sent.keys().cloned().collect();
        let conflicts = d.path().join("conflicts");
        for _ in 0..2 {
            let report =
                merge_files(None, &base, &sent, &refs, "extras/", &scopes, &conflicts).unwrap();
            assert_eq!(report.conflicts.len(), 2);
            assert_eq!(
                std::fs::read_to_string(base.join("deleted-remotely-edited-locally")).unwrap(),
                "local"
            );
            assert_eq!(
                std::fs::read_to_string(base.join("changed")).unwrap(),
                "remote"
            );
            assert_eq!(
                std::fs::read_to_string(base.join("conflict")).unwrap(),
                "local"
            );
            assert_eq!(
                std::fs::read_to_string(base.join("local-only")).unwrap(),
                "local"
            );
            assert_eq!(
                std::fs::read_to_string(conflicts.join("conflict")).unwrap(),
                "remote"
            );
            assert!(!base.join("deleted-local").exists());
            assert!(!base.join("deleted-remote").exists());
        }
    }
    #[test]
    fn destination_symlink_is_never_followed() {
        let d = tempfile::tempdir().unwrap();
        let base = d.path().join("base");
        std::fs::create_dir(&base).unwrap();
        let outside = d.path().join("outside");
        std::fs::write(&outside, "safe").unwrap();
        std::os::unix::fs::symlink(&outside, base.join("file")).unwrap();
        let entry = incoming(&d.path().join("incoming"), "file", "unsafe");
        assert!(
            merge_files(
                None,
                &base,
                &BTreeMap::new(),
                &[&entry],
                "extras/",
                &[],
                &d.path().join("conflicts")
            )
            .is_err()
        );
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "safe");
    }
}
