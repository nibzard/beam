# beam

<p align="center"><img src="docs/beam-cover.gif" alt="Beam: a Git commit graph beams up to a sandbox and back home" width="100%"></p>

Move your workspace to a sandbox, continue there, and bring the work home.

Beam sends your Git state (history, staged and unstaged changes, untracked files) and, optionally, your Claude Code session to a sandbox. The agent continues there. `beam down` brings the work back and never overwrites local work. Running databases, servers, and processes stay on your machine.

[Releases](https://github.com/steel-experiments/beam/releases) · [Design contract](SPEC.md) · [Contributing](CONTRIBUTING.md)

## Install

Download a release for macOS or Linux and put `beam` on your PATH:

```sh
VERSION=v0.2.0
TARGET=aarch64-apple-darwin   # or x86_64-apple-darwin, x86_64-unknown-linux-musl, aarch64-unknown-linux-musl
curl -fsSL "https://github.com/steel-experiments/beam/releases/download/$VERSION/beam-$VERSION-$TARGET.tar.gz" | tar -xz
sudo install "beam-$VERSION-$TARGET/beam" /usr/local/bin/beam
beam --version
```

Each release has a `SHA256SUMS` file. To check an archive, download it and run `shasum -a 256 -c SHA256SUMS --ignore-missing`.

Or build with Rust 1.89 or later:

```sh
cargo install --git https://github.com/steel-experiments/beam --tag v0.2.0 --locked
```

You also need Git, and one destination:

| Destination | What you need |
|---|---|
| Docker on this machine | Docker |
| Docker or a host over SSH | SSH access to the host |
| Steel | The [Steel CLI preview](#steel) and `STEEL_API_KEY` |
| Daytona | `DAYTONA_API_KEY`, curl, and OpenSSH |

To use Beam from Claude Code, also run this one time:

```sh
beam skill
```

It installs a `beam` skill in `~/.claude/skills`.

## Quickstart

### Let Claude Code beam itself up

In a Git project with at least one commit, tell your Claude Code session:

> beam up to steel, bypass permissions, and keep working on the parser tests

The skill moves this session to the sandbox, and the agent continues there at once. It asks for the destination or the permission mode if you do not give them. When it says that the move is complete, you can close the local session.

When you want the work back:

```sh
beam down
```

Beam applies the remote work and opens the session in Claude Code again. The agent gets a message that it is back on your machine.

### Or use the shell

```sh
cd ~/dev/app
beam build-image                 # Build the bundled Docker image; no project needed
beam --to docker --build-image   # Build the bundled image and send the workspace
beam attach                      # Open the remote terminal again
beam down                        # Bring the work home and remove the sandbox
```

1. Beam shows a plan: the project, the destination, the session, and what transfers, returns, and stays local.
2. Press **Enter** to send, or type `n` to cancel.
3. Beam opens the remote terminal. To detach, press **Ctrl-b**, then **d**.

The plan shows transfer scope, extra-file policies, and warnings before you confirm. Add `--details` to show environment names, setup commands, the session ID, and toolchain pins. `beam --dry-run` and `beam doctor` show these details too. Environment values stay hidden.

Without `--to`, Beam shows a menu of destinations and saves your choice as a personal default. Exit a local Claude session before you send it from the shell. Otherwise Beam stops, and `--force` overrides this check.

### Claude authentication

Beam cannot copy a macOS Keychain login. Run `claude setup-token` and export `CLAUDE_CODE_OAUTH_TOKEN` before you send a session. Beam forwards it, or another supported authentication variable. Otherwise, log in after `beam attach`.

## How it works

**What transfers**

- **Git:** complete history reachable from the current checkout, staged changes, unstaged changes, untracked files, deletions, executable bits, and symlinks.
- **Extras:** selected ignored files. `.env` and `.env.local` are included when present and ignored. They are **send only** by default. Use `return_extras` to bring their remote edits home.
- **Session:** Claude transcripts, project memory, and file history. User instructions, skills, agents, commands, and filtered settings are copied too.
- **Environment:** only variables listed in `[env] forward`, plus available Claude authentication variables when transferring Claude.

**What stays local:** ignored build output such as `node_modules/`, `target/`, and `.venv/`, running processes, and databases. Beam detects dependency commands from lockfiles and runs them in the sandbox. You can override them with `[sandbox] setup`.

The project keeps its absolute path in the sandbox. Plain SSH uses a private home directory for agent files, so it does not overwrite the host user's Claude configuration.

**What returns:** remote Git work, the session files, and `return_extras`. Local files stay editable while the workspace is remote. When both sides change, Beam combines separate file edits if branch history did not change. Other conflicts keep your local work in place and save the remote work in a recovery worktree.

## Destinations

| Target | Requirements |
|---|---|
| `docker` | Local Docker. Use `beam build-image` to build the bundled image. |
| `docker+ssh://HOST` | Docker on HOST. `beam build-image --to docker+ssh://HOST` builds the image there. |
| `ssh://HOST` | Git, tmux, tar, gzip, and the required project tools. The project path must not exist. Passwordless sudo may be needed to create its parent. |
| `daytona` | `DAYTONA_API_KEY`, local curl and OpenSSH. Beam creates a sandbox and installs base tools. |
| `daytona:SNAPSHOT` | The same, using an existing Linux snapshot with your project toolchain. |
| `steel` | Steel CLI preview and credentials. Beam creates a computer and installs the base tools. |
| `steel:CHECKPOINT` | The same, using a checkpoint that contains your project toolchain. |

`--to` overrides project `[beam] to`, which overrides your personal default. `beam default` shows the personal default, `beam default steel` changes it, and `beam default --clear` shows the menu again.

Docker and SSH transfer prerequisites are checked before allocation. Cloud providers check them after allocation and before upload. Git, tmux, archive tools, and the selected agent must be available. Missing transfer tools stop upload; the saved computer can be repaired and reused. Shell transfers also require project tools before upload. `beam doctor --to TARGET` runs the same checks without a transfer.

The bundled Docker image includes Node.js 22, pnpm 11.11.0 (through corepack, so a `packageManager` field selects its own version), Python, Git, tmux, and the latest Claude Code. Beam warns when the image is older than 30 days; rebuild it with `beam build-image`.

In Docker, Steel, and Daytona sandboxes, Beam installs missing Node.js, pnpm, cargo, and uv before setup. Project pins select versions; otherwise Node.js and pnpm match your machine. The plan shows what it installs. SSH hosts get no installations. Docker sandboxes keep package downloads and pnpm metadata in the `beam-cache` volume, so a repeated `pnpm install` reuses them. The registry can still be slow: pnpm checks cached metadata with it; set `[sandbox] cache = false` to opt out, and remove it with `docker volume rm beam-cache`.

For agent transfers, other missing project toolchains become an environment repair task. A custom image or Steel checkpoint can avoid that work. Numeric pins in `.nvmrc`, `.tool-versions`, `mise.toml`, `rust-toolchain.toml`, and `package.json` are checked for supported tools. Dynamic version aliases produce a warning.

### Steel

Install the Steel CLI preview and add it to your PATH:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/steel-dev/cli/releases/download/v0.5.0-preview.6/steel-cli-installer.sh | sh
export PATH="$HOME/.steel/bin:$PATH"
```

Run `steel --version` to verify the installation. Add the `export` line to your shell startup file to keep it available.

Set `STEEL_API_KEY` through your usual secret manager or shell environment. Steel computers pause after `[sandbox] timeout`, which defaults to `4h`. `beam attach` and `beam down` resume them.

If allocation is interrupted before Steel returns its computer ID, Beam does not allocate another computer automatically. Inspect `steel computer list --json`, then run `beam --recover-sandbox ID` with the computer from that transfer.

### Daytona

Set `DAYTONA_API_KEY` in your shell environment, then transfer:

```sh
beam --to daytona --agent shell
beam --to daytona:my-snapshot
```

Beam uses Daytona's REST API and OpenSSH; the Daytona CLI is optional. The default snapshot must support root access and Debian/Ubuntu package installation. If its SSH user is not root, Beam uses passwordless sudo for sandbox commands. Custom Linux snapshots can provide Git, tmux, curl, tar, gzip, bash, and the selected agent in advance. `[sandbox] image` and `--build-image` apply to Docker; choose a Daytona snapshot through the target.

`[sandbox] timeout` sets Daytona's inactivity auto-stop interval, rounded up to minutes. Auto-deletion and wall-clock TTL are disabled so stopped work remains available for return. `beam attach` and `beam down` start stopped or archived sandboxes. Stopping ends running processes; use `beam restart` to start a new session. Paused VM snapshots require manual resume before use. Beam waits for sandbox startup and confirms deletion before closing the transfer. If deletion fails or remains pending, retry `beam down` or `beam kill --yes`.

If allocation is interrupted, inspect the Daytona dashboard for the sandbox named `beam-TRANSFER_ID`, then run `beam --recover-sandbox ID`. Beam checks its transfer label before adopting or deleting it. It never automatically repeats an ambiguous allocation.

For a custom deployment, set `DAYTONA_API_URL` (including `/api`) and `DAYTONA_SSH_HOST` (hostname, port 22). SSH host keys use OpenSSH's `accept-new` policy and your usual known-hosts file.

## Working in the sandbox

### Let the agent work without you

By default, the remote Claude Code asks for approval as usual and can wait for you. Two options change this:

- `--permission-mode MODE` (or `[agent] permission_mode`) starts the remote Claude Code in that mode, for example `acceptEdits` or `bypassPermissions`. `bypassPermissions` runs as root with `IS_SANDBOX=1` and skips its confirmation dialog. Use it only in a sandbox.
- `--continue TEXT` adds a next step to the handoff and tells the agent not to wait for a reply.

The optional `[task]` fields in `beam.toml` also go into the handoff. They are your instructions. Beam does not infer an objective from the transcript or claim to verify `last_verified`.

### Publish work through GitHub

Enable the optional pull request (PR) workflow once per project. Beam then includes publication instructions in every sandbox handoff. You provide the task; you do not need to ask separately for commits, pushes, or a PR.

Add this configuration to `beam.toml`:

```toml
[workflow]
pr = true
```

Then use your normal transfer command:

```sh
beam up --to steel --continue "Fix the parser tests"
```

To enable the workflow for one transfer instead, add `--pr`:

```sh
beam up --to steel --pr --continue "Fix the parser tests"
```

Beam creates a unique `beam/TRANSFER_ID` branch in the sandbox. Each enabled handoff instructs the agent to:

- Make Conventional Commits for meaningful task changes and follow repository commit rules.
- Run relevant checks and record the results.
- Push completed commits regularly and verify that each push succeeds.
- Open or update a draft PR with a description and validation results.
- Report push failures and remaining uncommitted work before returning.

The agent must not force-push or merge the PR. These are agent instructions, not enforced publication guarantees. A task that produces no meaningful changes does not need a PR.

Both `--pr` and `[workflow] pr = true` enable GitHub authentication. Beam selects credentials in this order:

1. The `GH_TOKEN` environment variable.
2. The `GITHUB_TOKEN` environment variable.
3. Your local GitHub CLI (`gh`) login, through `gh auth token --hostname github.com`.

If you use the GitHub CLI, log in locally before the transfer:

```sh
gh auth login --hostname github.com
```

Authentication must be available before allocation. The token needs access to push the repository and create PRs. Token values are not printed or saved in transfer receipts. The sandbox receives the token through Beam's private environment file.

Use `--github-auth` to forward authentication without enabling the PR workflow. Both options require a GitHub.com `origin`. In the sandbox, SSH origins use HTTPS and a repository-local `gh` credential helper. Your local origin is unchanged. Beam does not copy SSH private keys or your full GitHub configuration.

The bundled image and cloud bootstrap include `gh`. Rebuild an existing Docker image with `beam build-image`. Custom images and SSH hosts need `gh`; agent sessions can receive missing-tool repair instructions.

For authentication on every transfer without the PR workflow, use:

```toml
[workflow]
github_auth = true
```

At `beam down`, Beam reports uncommitted paths, whether the branch's current commit matches GitHub, and a PR URL when available. Network checks have time limits. Failed checks report unknown evidence and do not block recovery. Publication evidence is saved as `publication.txt` in the transfer receipt.

Beam still returns commits, staged and unstaged changes, untracked files, and session files. A return can select the new task branch locally. Beam does not make a commit or push during return.

A successful push preserves committed code after sandbox deletion. It does not preserve uncommitted changes or agent sessions. Beam does not schedule a final push before expiry or save periodic external checkpoints. Push regularly during the task; download the return package before the sandbox is deleted.

### The beam skill

From a local session, the skill runs `beam -y -d --force` for this session, with the destination, the permission mode, and a next step from the conversation. After the move, the local session writes one last reply. On `beam down`, the remote conversation replaces that local copy, but only when the local copy has no new messages from you. If you write more messages locally after the move, the usual conflict rule keeps the local conversation. `beam undo` restores the local copy.

Claude Code in the sandbox has its own `beam` skill. When you ask it to beam down, it runs `beam down` and finishes its reply before the session stops. It does not return the work only because the task looks complete.

### Commands in the sandbox

The sandbox also has a `beam` command. Use it from the agent terminal, from `docker exec`, or from the Steel web terminal.

| Command | Behavior |
|---|---|
| `beam attach [ID]` | Open the agent terminal. Two terminals can share it. |
| `beam down [ID]` | Stop the agent and pack its work. Then run `beam down` (or keep `beam down --wait` running) on your local machine. Sandbox edits after the pack do not return. |
| `beam ls` | List the transfers in the sandbox |

### Status and monitoring

`beam status` shows the transfer and one next command. It separates process state from task evidence: a running process does not prove authentication, progress, or completion. JSON includes `agent`, `capabilities`, `process`, `task`, source-labeled `observations`, `summary`, `next_action`, and `saved_recovery`.

Agents can report progress from the remote session:

```sh
sh "$BEAM_REPORT" waiting "Need a decision about the redirect"
sh "$BEAM_REPORT" finished "Changed the callback; the redirect test passes"
```

Reports support `working`, `waiting`, `finished`, and `failed`. They are labeled as agent reports. A process exit or idle terminal does not prove task completion.

The Claude adapter also checks the visible terminal footer as a fallback. A recognized English confirmation or permission footer, such as "Enter to continue · Esc to cancel", reports `possible-input` and recommends `beam attach`. JSON labels this evidence `terminal-heuristic`. Agent reports take priority. Beam does not inspect scrollback, send input, or expose pane text. Unknown dialogs may not be detected.

`beam status --watch --notify` rings the terminal bell for new attention reports, completion reports, failures, process exits, and sandbox packs. It also alerts once for a live input prompt, including a prompt present when watching starts. After the prompt clears, a later prompt can alert again. The watcher must stay running and sends no external messages. `--interval SECONDS` controls polling; `--count N` limits polls. With `--json`, each changed status is a JSON object.

Setup and check events include elapsed seconds. Transfer receipts also record time between saved phases, including time spent waiting for you. `beam review --json` includes the plan, file actions, task record, events, and timings.

### Setup checks and environment repair

Beam checks project tools, runs setup, and runs `[sandbox] verify` in the session directory. If a check fails, Claude starts with environment repair instructions. The handoff includes required tools and versions, setup and verification commands, and recent failure output. Full output remains in `beam logs`.

During repair, status shows **environment repair** (`process: repairing` in JSON). Use `beam attach` for authentication or other input. The agent must keep project version requirements and checks. It runs `sh "$BEAM_CHECK"` after repairs to rerun Beam's saved checks. Only successful checks clear repair status. Agent completion reports do not clear it. Empty verification leaves project readiness unverified even after setup passes.

If a repair agent exits before checks pass, Beam keeps an inspection shell available and reports unfinished repair, even when the agent's exit code is zero. After repair, detach and run `beam restart` locally to rerun checks and resume the agent.

Agents without environment repair, including `--agent shell`, keep manual recovery. Failed setup or verification records `needs-attention` and opens a shell. Fix the environment, then run `beam` again. A missing agent executable blocks upload.

`reuse_setup = true` reuses successful setup only within the same sandbox, for example after `beam restart`. It requires verification commands. Beam fingerprints workspace content, the launcher, forwarded environment, tools, and `setup_inputs`. It always reruns verification. List ignored setup inputs explicitly. Missing inputs or changed fingerprints disable reuse. Verification failure invalidates the saved setup result. A new sandbox still runs setup.

## Bringing work home

`beam down` stops the remote agent, downloads its work, applies it, and removes the sandbox. One summary line shows the commits and paths that returned, the time away, and conflicts.

After a clean return in a terminal, Beam opens the session in Claude Code again. The agent's first prompt tells it that it is back, that sandbox tools are not here, and that it must wait for you. Beam prints the command instead, with the same message, when there are conflicts, with `beam down -d`, without a terminal, or when Claude Code already runs in the project.

- `beam down --keep` returns the work and keeps the sandbox for inspection. Later sandbox edits do not return.
- `beam down --wait` waits until `beam down` runs in the sandbox, then returns the work.

### Review and undo

To inspect work before you apply it:

```sh
beam down --review
beam review --diff
beam review --apply
```

The return snapshot is fixed when downloaded. Further remote edits are not included. The review shows changed paths, conflicts, returning extras, and saved worktree paths. `beam review --open` opens a shell in the remote copy. Exit that shell to return.

Beam combines changes to separate paths when both sides keep the original commit and branch. It keeps staged and unstaged changes apart. Different changes to the same path, file/directory collisions, local ignored file collisions, and diverging history require manual integration. A local-only change stays local. Identical states are kept once.

The proposed result is an inspection copy. Editing it does not change the saved apply plan. For conflicts, run `beam down` to finish saving recovery and remove the sandbox. Then integrate the saved remote work through your editor or Git. Run `beam review --resolved` when finished. This records your decision; it does not merge files or delete copies.

If Git state changes after review, Beam rebuilds the plan and stops for another review. If an extra or agent file changes, use `beam review --refresh`. Refresh is available before apply starts.

`beam undo` restores the pre-return Git state, extras, and agent files changed by the return. It checks for later edits first. When later edits conflict, Beam keeps them and points to saved copies. Undo does not change the sandbox. Review and undo select the latest saved return for this project. Use `--transfer ID` to select an older receipt.

### Recovery

- **Upload interrupted:** run `beam` again. Completed phases are reused.
- **Environment repair:** use `beam attach` to inspect the agent and supply any needed authentication. The agent reruns Beam checks after repair.
- **Manual setup failed:** inspect `beam logs`, fix the environment through `beam attach`, then run `beam` again.
- **Download or cleanup interrupted:** run `beam down` again. A completed local apply is not repeated.
- **Conflicting work:** run `beam review` to inspect the saved return. Your local changes remain in place.
- **Sandbox retained:** use `beam attach` to inspect it, then `beam kill --yes` to remove it. Further sandbox edits will not return through `beam down`. Running `beam down` again removes the retained sandbox without importing those edits.

If you lose the recovery path, run `beam status`. It shows the latest closed receipt with saved conflicts for this project, even during a later transfer. Beam cannot detect a manual merge. Run `beam review --resolved --transfer ID` to record it without removing the receipt.

Beam saves transfer receipts and return packages in `~/.beam/transfers/`. Recovery worktrees live there too. These records can contain transcripts and secrets. Their directories are private. Keep the receipt until you have verified your returned work. To remove a recovery copy that you no longer need, use `git worktree remove --force PATH` with its printed path. Then delete that closed transfer's receipt directory. Do not delete an active transfer record.

## Configuration

Personal defaults live in `~/.config/beam/config.toml`, or under `XDG_CONFIG_HOME`:

```toml
# ~/.config/beam/config.toml
to = "docker+ssh://devbox"
```

Project settings live in `beam.toml`:

```toml
[beam]
to = "steel"                           # Project default destination

[files]
extras = [".env", "config/dev.json"]   # Send only
return_extras = ["config/local-state"] # Send and return, including deletions
max_file_size = "50MB"

[env]
forward = ["DATABASE_URL"]

[sandbox]
image = "beam-base:latest"
timeout = "4h"                        # Steel pause / Daytona idle stop
setup = ["pnpm install --frozen-lockfile"]
verify = ["pnpm test"]                 # Checked before normal task work
reuse_setup = true                     # Same sandbox only; requires verify
setup_inputs = [".env"]                # Extra ignored inputs that affect setup
cache = true                           # Docker: share package downloads in the beam-cache volume

[agent]
permission_mode = "acceptEdits"       # Remote Claude Code permission mode

[task]
objective = "Fix the login redirect"
complete_when = "The redirect test passes"
last_verified = "The test reproduces the failure"
next_action = "Inspect the callback handler"
constraints = ["Keep the public API"]
```

Extras must be ignored by Git. `return_extras` paths are also uploaded when present. Missing return paths may be created remotely. Returning files use their original hashes to detect local changes. Beam keeps both versions when both sides change.

The size limit covers current Git files, staged blobs, extras, and transferred agent files. Historical Git objects still transfer; the preview states that complete reachable history is included. `--allow-large` permits files above the limit.

Setup commands replace detection and must be safe to repeat after failure.

## Command reference

| Command | Behavior |
|---|---|
| `beam [PATH]` or `beam up [PATH]` | Send the workspace or continue an interrupted upload |
| `beam --dry-run --to TARGET` | Preview and check a transfer without creating it |
| `beam doctor --to TARGET` | Check the same prerequisites used by upload |
| `beam build-image [--to docker+ssh://HOST] [--image TAG]` | Build the bundled Docker image without a project or transfer |
| `beam demo [--down]` | Preview a text animation without a sandbox or credentials |
| `beam attach [PATH]` | Open the agent or a repair shell |
| `beam status [--json]` | Show transfer progress, remote readiness, saved recovery, and one next action |
| `beam status --watch [--notify]` | Watch evidence and optionally ring the terminal bell for new attention or exit events |
| `beam logs` | Show setup logs and recent terminal output |
| `beam ls [--json]` | List active and retained transfers |
| `beam restart` | Restart a stopped session in its existing sandbox and rerun project checks |
| `beam down [PATH]` | Return work, save recovery data, remove the sandbox, and open the local session (`-d` prints the command instead) |
| `beam down --keep` | Return work and keep the sandbox listed and manageable |
| `beam down --wait` | Wait for `beam down` in the sandbox, then return the work |
| `beam down --review` | Download a fixed return snapshot for review; stop the remote session and leave local project files unchanged |
| `beam review [--diff / --json / --open]` | Inspect the latest saved return, or open a shell in its remote worktree |
| `beam review --apply [--keep]` | Apply the reviewed plan after checking for later local edits |
| `beam review --refresh` | Rebuild an unapplied plan after changing local files |
| `beam review --resolved` | Mark manual recovery resolved and keep saved copies |
| `beam undo` | Restore the state before return when later edits do not conflict |
| `beam default [TARGET / --clear]` | Show, set, or clear your personal default destination |
| `beam skill` | Install the Claude Code skill that moves a local session with `beam` |
| `beam kill --yes` | Remove the sandbox without returning additional work |
| `beam forget --yes` | Remove the active record after you manually remove a lost sandbox |

Options for `beam up`:

| Option | Behavior |
|---|---|
| `--to TARGET` | Destination for this transfer |
| `--session ID` | Session to send. The default is the session that changed last. Interactive mode offers a choice when several exist. |
| `--agent shell` | Send a workspace without an agent. With no Claude session, the default `auto` opens a shell. |
| `--permission-mode MODE` | Permission mode for the remote Claude Code |
| `--continue TEXT` | Next step for the remote agent, which then does not wait for a reply |
| `-y`, `--yes` | Ask no questions. Needs a destination from `--to` or a default. |
| `-d`, `--detach` | Do not open the remote terminal after the move |
| `--force` | Send the session also when Claude Code still runs in the project |
| `--allow-large` | Upload files larger than `max_file_size` |
| `--pr` | Create a task branch and instruct the agent to commit, push, and open a draft PR; includes GitHub authentication |
| `--github-auth` | Forward GitHub authentication for Git and `gh` without the PR workflow |
| `--details` | Show environment names, setup commands, the session ID, and toolchain pins |
| `--build-image` | Build the bundled Docker image first |
| `--recover-sandbox ID` | Adopt a cloud sandbox after an interrupted allocation |

`beam me up` and `beam me down` also work.

## Terminal effects

Progress text keeps the terminal's text color. The adjacent symbol animates; the action and elapsed time stay readable. Long animated lines shorten to fit the window. Each completed step leaves its result and elapsed time in scrollback. Static output wraps normally and prints a completion line too.

During the remote part of `beam` and `beam down`, the `beam demo` animation plays in ten rows above the progress line: the workspace moves out, turns as a cloud of dots while the work runs, and forms at the destination when the work completes. Other output prints above it, and Beam erases it at the end, so scrollback keeps only the progress lines. Windows smaller than 40 columns or 20 rows keep the spinner only.

During slow transfer steps, Beam keeps the inline progress display and all previous output in scrollback. On Ghostty and Kitty, a faint transparent image moves beneath the active line after 600 ms. The image contains no text, adds no transcript rows, and is removed when the step ends or prints another message. Unknown terminals, small windows, tmux/screen, piped output, `TERM=dumb`, and `NO_COLOR` keep the plain display. Beam does not query or consume terminal input to detect graphics support; it uses `TERM_PROGRAM` and `KITTY_WINDOW_ID`, and suppresses graphics protocol replies.

Set `BEAM_EFFECT=off` to keep only the inline spinner. Set `BEAM_ANIMATION=0` to disable all motion: the spinner, the transfer animation, ambient graphics, shader activation, the demo, and the animated tab indicator. Progress then uses static lines with elapsed time. Colors and completion notifications remain available. `BEAM_EFFECT=graphics` selects the image effect on the supported terminals; the default is `auto`. The image occupies one row, so it does not reserve space or move existing output. Resizing so that the status line no longer fits disables the effect for that step.

### Text demo

Run the demo from any directory:

```sh
beam demo
beam demo --down
BEAM_ANIMATION=0 beam demo
```

The demo shows a voxel workspace dispersing, moving, and forming again. It uses colored Unicode braille characters. It needs no graphics protocol or shader configuration. It opens a temporary full-screen display; real transfers keep their inline display and scrollback.

Press **Esc** or **q** to exit. **Ctrl-C** interrupts the demo. It restores the cursor, wrapping, input settings, and original screen. Use `--seconds 1` through `--seconds 30` to change the duration. Windows smaller than 40 columns or 12 rows get a static preview. If you resize during playback, the demo adapts or shows a resize hint. Pipes, `NO_COLOR`, `TERM=dumb`, and `BEAM_ANIMATION=0` get static text without full-screen control sequences.

See the [recorded examples](docs/demos/README.md) for the demo, saved-return review, undo, and conflict recovery.

### Optional Ghostty shader

For a full-window effect in Ghostty 1.3 or later, an optional shader is included at `shaders/beam.glsl`. Copy it to a location of your choice and add its absolute path to your Ghostty configuration:

```ini
custom-shader = /absolute/path/to/beam.glsl
custom-shader-animation = true
```

Reload Ghostty's configuration, then run `BEAM_EFFECT=shader beam --to steel`. The shader adds slow, faint shafts of light only during slow Beam steps, with reversed motion on return. It keeps text pixels, selection colors, and terminal alpha. No configuration is changed automatically.

Shader activation temporarily sets the cursor color to a Beam marker and resets it to the theme color on exit, cancellation, or intervening output. If another program had dynamically changed the cursor color, that color is not kept; use the image effect instead in that case. Without the shader installed, this mode only changes the cursor color. An abrupt kill such as SIGKILL cannot run cleanup; reset the cursor with `printf '\033]112\033\\'`.

Preview either effect without a sandbox:

```sh
cargo run --example ambient
BEAM_EFFECT=shader cargo run --example ambient
cargo run --example ambient -- show   # the transfer animation above the progress line
cargo run --example ambient -- down
```

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
python3 scripts/test_demo.py
python3 scripts/test_ambient.py
BEAM_E2E_TARGET=docker cargo test --test e2e -- --ignored
cargo test --test e2e_steel -- --ignored
cargo test --test e2e_daytona -- --ignored
```

Docker tests use a fixture agent. Daytona tests require an API key and an authenticated Daytona CLI to verify a real shell round trip. Steel tests verify transport and return using a real computer; they do not verify an authenticated Claude conversation. See [CONTRIBUTING.md](CONTRIBUTING.md) for validation and releases, and [SPEC.md](SPEC.md) for the implemented contract.

Run `cargo run --release --locked -- demo --benchmark` to measure software rendering and encoding at three terminal sizes. The benchmark does not draw to the terminal or allocate a sandbox. The renderer remains internal and adds no dependencies.

## License

MIT. See [LICENSE](LICENSE).
