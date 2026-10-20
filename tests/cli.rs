// ABOUTME: Local command regressions. These run without a provider or credentials.
mod common;
use common::{Env, text};

#[test]
fn transfer_preview_hides_details_but_keeps_scope_and_warnings() {
    let env = Env::new(
        "[sandbox]\nsetup = ['echo preparing-example']\nverify = ['git diff --check']\n[env]\nforward = ['BEAM_E2E_VAR']\n",
    );
    for flags in [vec![], vec!["--details"], vec!["--dry-run"]] {
        let mut args = vec![
            "up", "--to", "daytona", "--agent", "shell", "--yes", "--detach",
        ];
        args.extend(&flags);
        let output = env
            .command(&args)
            .env_remove("DAYTONA_API_KEY")
            .output()
            .unwrap();
        assert!(!output.status.success());
        let output = text(&output);
        for label in [
            "project",
            "destination",
            "Send:",
            "Return:",
            "Stay local:",
            ".env",
            "DAYTONA_API_KEY",
        ] {
            assert!(output.contains(label), "missing {label}: {output}");
        }
        assert!(
            !output.contains("forwarded-value"),
            "environment value leaked: {output}"
        );
        if flags.is_empty() {
            assert!(!output.contains("echo preparing-example"));
            assert!(!output.contains("BEAM_E2E_VAR"));
            assert!(output.contains("1 project checks"));
            assert!(output.contains("add --details"));
        } else {
            assert!(output.contains("echo preparing-example"));
            assert!(output.contains("BEAM_E2E_VAR"));
            assert!(output.contains("git diff --check"));
        }
    }
}

#[test]
fn demo_needs_no_project_and_plain_output_does_not_own_the_terminal() {
    let directory = tempfile::tempdir().unwrap();
    for flags in [vec![], vec!["--down"], vec!["--benchmark"]] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_beam"))
            .arg("demo")
            .args(&flags)
            .current_dir(directory.path())
            .env("TERM", "dumb")
            .env("BEAM_ANIMATION", "0")
            .env_remove("STEEL_API_KEY")
            .env_remove("DAYTONA_API_KEY")
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", text(&output));
        assert!(!output.stdout.contains(&0x1b));
        if flags == ["--benchmark"] {
            assert!(text(&output).contains("160x50:"));
        } else {
            assert!(text(&output).contains("no files are transferred"));
            assert!(text(&output).contains(if flags.is_empty() {
                "Moving the workspace to the sandbox"
            } else {
                "Bringing the workspace home"
            }));
        }
    }
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn build_image_needs_no_project_and_rejects_targets_without_docker() {
    let directory = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_beam"))
        .args(["build-image", "--to", "steel"])
        .current_dir(directory.path())
        .env("TERM", "dumb")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        text(&output).contains("build-image needs a Docker target"),
        "{}",
        text(&output)
    );
}

#[test]
fn build_image_flag_with_a_cloud_target_fails_before_the_plan() {
    let env = Env::new("");
    let output = env
        .command(&[
            "up",
            "--to",
            "steel",
            "--agent",
            "shell",
            "--yes",
            "--build-image",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let output = text(&output);
    assert!(
        output.contains("--build-image needs a Docker target"),
        "{output}"
    );
    assert!(output.contains("beam build-image"), "{output}");
    assert!(
        !output.contains("destination"),
        "the plan was shown: {output}"
    );
}

#[test]
fn staged_file_and_extra_size_limits_run_before_provider_access() {
    let env = Env::new("[beam]\nto = 'ssh://not-used'\n[files]\nmax_file_size = '1KB'\n");
    std::fs::write(env.project.join("large"), vec![b'x'; 2048]).unwrap();
    common::sh(&env.project, "git add large");
    let out = env.beam(&["--yes", "--detach"]);
    assert!(!out.status.success());
    assert!(
        text(&out).contains("these files are larger"),
        "{}",
        text(&out)
    );
    common::sh(&env.project, "git reset -q -- large && rm large");
    std::fs::write(env.project.join(".env"), vec![b'x'; 2048]).unwrap();
    let out = env.beam(&["--yes", "--detach"]);
    assert!(!out.status.success());
    assert!(
        text(&out).contains("these files are larger"),
        "{}",
        text(&out)
    );
    assert!(!env.project.join(".beam/state.json").exists());
}

#[test]
fn missing_target_explains_noninteractive_setup() {
    let env = Env::new("");
    let out = env.beam(&["--yes", "--detach"]);
    assert!(!out.status.success());
    assert!(
        text(&out).contains("Supply the option explicitly"),
        "{}",
        text(&out)
    );
}

#[test]
fn personal_default_is_shown_and_explained_when_its_check_fails() {
    let env = Env::new("");
    let out = env.beam(&["default"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("no personal default"), "{}", text(&out));
    assert!(!env.beam(&["default", "nowhere"]).status.success());
    let out = env.beam(&["default", "daytona"]);
    assert!(out.status.success(), "{}", text(&out));
    let config = env.home.join(".config/beam/config.toml");
    assert_eq!(
        std::fs::read_to_string(&config).unwrap(),
        "to = \"daytona\"\n"
    );
    assert_eq!(text(&env.beam(&["default"])).trim(), "daytona");

    let out = env
        .command(&["doctor", "--agent", "shell"])
        .env_remove("DAYTONA_API_KEY")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let out = text(&out);
    assert!(out.contains("daytona (personal default, "), "{out}");
    assert!(out.contains("DAYTONA_API_KEY"), "{out}");
    assert!(out.contains("`beam default TARGET`"), "{out}");

    let out = env
        .command(&["doctor", "--agent", "shell", "--to", "daytona"])
        .env_remove("DAYTONA_API_KEY")
        .output()
        .unwrap();
    assert!(
        !text(&out).contains("beam default TARGET"),
        "{}",
        text(&out)
    );

    assert!(env.beam(&["default", "--clear"]).status.success());
    assert!(text(&env.beam(&["default"])).contains("no personal default"));
}

#[test]
fn unsafe_extra_path_is_rejected_before_provider_access() {
    let env = Env::new("[beam]\nto = 'docker'\n[files]\nextras = ['../secret']\n");
    let out = env.beam(&["--yes", "--detach"]);
    assert!(!out.status.success());
    assert!(text(&out).contains("relative path"), "{}", text(&out));
}

#[test]
fn explicit_missing_session_never_falls_back_to_a_shell() {
    let env = Env::new("[beam]\nto = 'docker'\n");
    let out = env.beam(&["--yes", "--detach", "--session", "missing"]);
    assert!(!out.status.success());
    assert!(
        text(&out).contains("session missing not found"),
        "{}",
        text(&out)
    );
}

fn receipt(env: &Env, id: &str, phase: &str, created: u64, conflicts: bool) -> std::path::PathBuf {
    let dir = env.home.join(".beam/transfers").join(id);
    std::fs::create_dir_all(dir.join("worktree")).unwrap();
    let record = serde_json::json!({
        "version": 2, "transfer_id": id, "session_id": "test", "agent": "shell",
        "project_root": env.project, "agent_cwd": env.project, "home": env.home,
        "target": "docker", "sandbox": null, "stage": "", "tmux": "test",
        "sent": {"head":"", "branch":"", "idx_tree":"", "wt_tree":"", "wt_commit":""},
        "sent_files": {}, "phase": phase, "created_at": created,
        "recovery": dir.join("worktree"),
        "conflicts": if conflicts {vec!["remote work saved separately"]} else {vec![]}
    });
    std::fs::write(dir.join("state.json"), serde_json::to_vec(&record).unwrap()).unwrap();
    dir
}

#[test]
fn status_finds_latest_saved_recovery_without_hiding_active_transfer() {
    let env = Env::new("");
    receipt(&env, "old", "closed", 1, true);
    let latest = receipt(&env, "latest", "closed", 2, true);
    receipt(&env, "clean", "closed", 3, false);
    let json = env.beam(&["status", "--json"]);
    assert!(json.status.success(), "{}", text(&json));
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(value["phase"], "local");
    assert_eq!(
        value["saved_recovery"]["receipt"],
        latest.to_string_lossy().as_ref()
    );
    assert!(value["next_action"].as_str().unwrap().starts_with("cd "));
    assert!(!env.project.join(".beam/state.json").exists());

    receipt(&env, "active", "applied", 4, false);
    let out = env.beam(&["status"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("Sandbox cleanup remains"));
    assert!(text(&out).contains(&latest.display().to_string()));
    assert_eq!(text(&out).matches("Next:").count(), 1);
    assert!(text(&out).contains("Next: beam down"));
}

#[test]
fn recovery_receipts_are_scoped_to_the_project_and_remain_read_only() {
    let env = Env::new("");
    let dir = receipt(&env, "other-project", "closed", 1, true);
    let path = dir.join("state.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["project_root"] = "/another/project".into();
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let before = std::fs::read(&path).unwrap();
    let out = env.beam(&["status", "--json"]);
    assert!(out.status.success(), "{}", text(&out));
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(value["saved_recovery"].is_null());
    assert_eq!(value["next_action"], "beam");
    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[test]
fn pr_preview_shows_workflow_and_hides_github_credentials() {
    let env = Env::new("");
    common::sh(
        &env.project,
        "git remote add origin git@github.com:acme/project.git",
    );
    let output = env
        .command(&[
            "up",
            "--to",
            "daytona",
            "--agent",
            "shell",
            "--pr",
            "--dry-run",
        ])
        .env("GH_TOKEN", "fixture-token-must-stay-hidden")
        .env_remove("DAYTONA_API_KEY")
        .output()
        .unwrap();
    let output = text(&output);
    assert!(output.contains("Conventional Commits"), "{output}");
    assert!(
        output.contains("authentication forwarded for Git and gh"),
        "{output}"
    );
    assert!(
        !output.contains("fixture-token-must-stay-hidden"),
        "{output}"
    );
    assert!(!env.project.join(".beam/state.json").exists());
    assert_eq!(
        common::sh(&env.project, "git branch --show-current").trim(),
        "main"
    );
}

#[test]
fn github_auth_rejects_non_github_origins_before_allocation() {
    let env = Env::new("");
    common::sh(
        &env.project,
        "git remote add origin https://example.com/acme/project.git",
    );
    let output = env
        .command(&["up", "--to", "daytona", "--github-auth", "--yes"])
        .env("GH_TOKEN", "fixture-token-must-stay-hidden")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(text(&output).contains("origin on github.com"));
    assert!(!env.project.join(".beam/state.json").exists());
}

#[test]
fn configured_pr_workflow_uses_local_gh_login_without_printing_its_token() {
    use std::os::unix::fs::PermissionsExt;
    let env = Env::new("[workflow]\npr = true\n");
    common::sh(
        &env.project,
        "git remote add origin https://github.com/acme/project.git",
    );
    let bin = env.home.join("fixture-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let gh = bin.join("gh");
    std::fs::write(&gh, "#!/bin/sh\n[ \"$*\" = 'auth token --hostname github.com' ] || exit 2\necho keychain-fixture-token\n").unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let output = env
        .command(&["up", "--to", "daytona", "--agent", "shell", "--dry-run"])
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .env_remove("DAYTONA_API_KEY")
        .output()
        .unwrap();
    let output = text(&output);
    assert!(output.contains("Conventional Commits"), "{output}");
    assert!(output.contains("GH_TOKEN"), "{output}");
    assert!(!output.contains("keychain-fixture-token"), "{output}");
}
