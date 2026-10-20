# Beam implementation contract

This document describes implemented behavior. Future ideas live in the final section.

## Product scope

Beam moves a Git workspace between a local machine and a sandbox. A Claude Code session is optional. The default agent mode resumes an existing session or opens a shell when none exists.

The transfer includes the current checkout's reachable history, index, worktree, selected ignored files, and agent files. It recreates dependencies through setup commands. It does not copy process memory, running services, databases, or installed toolchains.

Supported source platforms are Linux and macOS. The repository needs at least one commit. Submodules are rejected before transfer. Detected Git LFS configuration is rejected because its separate object store does not transfer. Windows is not supported.

## User workflow

`beam` builds a plan, checks prerequisites, asks for confirmation, and sends the workspace. A sandbox `beam down` can start the return from the sandbox (see Commands in the sandbox). The send confirmation uses `[Y/n]`: Enter accepts, and `n` cancels. Interactive mode attaches after startup. `--detach` returns to the local shell.

When no target is configured, interactive mode shows a numbered destination menu and saves the choice as a personal default. The menu marks Docker, the Steel CLI, and `DAYTONA_API_KEY` as found or missing, and the first ready one is the default. The user can also type a full target. The SSH choices ask for a host. Noninteractive commands must supply a target or configured default. Target precedence is `--to`, project `[beam] to`, then personal `to`. The plan shows where a configured target comes from. When the checks for a configured target fail, the error tells how to use a different target. `beam default` shows the personal default, `beam default TARGET` saves it, and `beam default --clear` removes it.

`--permission-mode MODE` starts the remote agent in that permission mode. Project `[agent] permission_mode` supplies a default. Only an agent session can use it, and the mode must be a word of ASCII letters. For Claude, `bypassPermissions` adds `IS_SANDBOX=1`, because sandboxes run as root, and a `--settings` value that skips the bypass confirmation dialog. Other modes pass through unchanged. `--continue TEXT` adds the next step to the handoff and tells the agent not to wait for a reply. The plan shows both values.

`beam skill` installs a Claude Code skill in `~/.claude/skills/beam/SKILL.md`. It does not replace a different skill at that path. The skill lets a local session run `beam -y -d --force --session ID` with a target, a permission mode, and a next step. After the move, it writes one reply and tells the user that they can close the session.

Claude sessions are discovered in the invocation directory, with a project-root fallback for automatic selection. Interactive mode offers a choice when several sessions exist. Noninteractive mode uses the latest modified transcript. Explicit session selection fails if the session is absent.

The plan leads with the project, destination, and selected session. It shows transfer and return policies, extra-file policies, changed-path counts, permission mode, continuation instructions, and warnings before confirmation. Normal transfers show counts for environment variables, setup commands, and project checks. `--details`, `--dry-run`, and `beam doctor` show environment names, command text, the session ID, unpushed-commit counts, and toolchain pins. Values of environment variables are not printed. Details do not change transfer inputs or checks.

`beam --dry-run` and `beam doctor` use the same plan and provider checks. They do not create a transfer sandbox. Docker checks use a temporary container that is removed after checking.

## State and ownership

The authoritative record is `~/.beam/transfers/<transfer>/state.json`. `<project>/.beam/state.json` points to it. `beam ls` derives its list from authoritative records. If the project pointer is missing, Beam finds the active record by project path. Mutating commands restore the pointer.

Record writes use a temporary file, file synchronization, atomic rename, and directory synchronization. Each project has an operating-system file lock. The lock releases when the process exits, including a crash. It serializes Beam commands; it does not lock editors or local agents.

Records use schema version 2. Unsupported versions fail explicitly. Version 1 records migrate on their next save. Old SSH transfers have no verifiable ownership markers, so automatic deletion is disabled for them.

The upload phases are:

```text
planned → allocating → created → prepared → uploaded → restored → starting → remote
```

The return phases are:

```text
remote → returning → downloaded → applied → closed
                                      └→ retained → closed
```

Errors retain the last completed phase and an error message. Repeating the corresponding command continues the transfer.

If the Steel connection closes during the archive upload, the remote command stops with exit 141 (SIGPIPE). Beam shows a warning and uploads the archive one more time, because extracting it again is safe. A second failure is saved as an error. Failed Steel commands show the exit code, the cause if Beam knows it, and the end of the remote output.

Status and upload results share the same interpretation of saved phases and live remote state. Each summary recommends one next command. Return and retention phases take precedence over remote process readiness. Status JSON retains the operation phase and adds `summary`, `next_action`, and `saved_recovery`.

Status also finds the latest closed receipt with conflicts for the current project. It shows saved recovery even when another transfer is active. This lookup does not modify receipts. Beam cannot detect manual conflict resolution. `beam review --resolved` writes a separate acknowledgment. Status omits acknowledged recovery, including retained transfers. Saved copies remain intact.

Docker sandboxes mount the shared `beam-cache` volume at `/var/cache/beam`, and the pnpm store and metadata, npm, yarn, bun, uv, and pip caches use it. Later sandboxes reuse downloaded packages. The volume is shared by all Docker transfers on that Docker host, and Beam does not remove it. `[sandbox] cache = false` disables it for a project. The plan shows when the cache applies.

The bundled image pins Node.js 22 and pnpm 11.11.0 through build arguments and installs the latest Claude Code. Corepack provides pnpm without a download prompt, so a project `packageManager` field selects its own pnpm version. `beam build-image` builds the bundled image without a project or transfer; `--to docker+ssh://HOST` builds it on that host. `--build-image` on a transfer does the same before the check, and fails before the plan when the target is not Docker. Both record the build time in the `beam.built` label. The Docker check warns when the default image is older than 30 days or has no build time label.

Docker allocation uses a deterministic container name and transfer label. An existing container must have the expected label. Steel allocation writes its response to a durable receipt. If the response is ambiguous, the user identifies the computer with `--recover-sandbox ID`; Beam does not allocate a duplicate automatically.

SSH preparation claims new directories with ownership markers. Existing unowned paths are rejected. Cleanup checks markers before removing paths. Agent files use a private home under the transfer stage on SSH. Shared user files are not overwritten or removed.

## Git snapshots

`scripts/snapshot.sh` runs both locally and remotely. It creates an index commit and a worktree commit using a temporary Git index. The worktree commit has the original HEAD and index commit as parents.

Upload bundles all objects reachable from the worktree commit. Return bundles exclude objects reachable from the sent snapshot. The branch, HEAD, index tree, and worktree tree are recorded separately.

Restore reconstructs the checkout, worktree, and index. Staged and unstaged changes remain distinct. Git preserves executable bits, not all filesystem permission bits. Symlinks in Git preserve their link targets; external targets do not transfer.

Before applying returned Git changes, Beam saves a separate recovery worktree. It compares the current local snapshot with the sent snapshot. Separate path changes can merge when both sides retain the sent commit and branch. Different changes to the same path or diverging history keep remote work in the recovery worktree. Local ignored files that would be overwritten also prevent apply. If local state already matches the returned state, a retry skips the apply.

Recovery worktrees preserve returned HEAD, index, worktree, and untracked files. They use a detached checkout and do not overwrite user branches or stashes.

## Archive and file policies

The outgoing archive contains the Git bundle, remote scripts, selected extras, agent files, filtered defaults, and a manifest. Environment values travel separately through standard input into a private remote file.

The same transfer plan determines the preview, size checks, and file inputs. The configured size limit covers current Git files, staged blobs, extras, and agent files. Historical objects are not individually limited. The archive size is shown after packing.

Default extras are existing ignored `.env` and `.env.local` files. Other extras must be specified explicitly and ignored by Git. Paths must be relative and cannot include parent traversal. Extras cannot be symlinks. User configuration links within HOME are copied as regular content. A link back into a directory that Beam is already copying adds no files: Beam skips it and shows a warning. Another tool can remove a user configuration file after the plan is shown. Beam does not send that file and shows a warning. A session file that is removed stops the upload, and the error names the file.

`extras` are send-only. `return_extras` are sent and returned. Return paths can include directories and can be absent before upload.

Returning extras use a three-way comparison:

| Local change | Remote change | Result |
|---|---|---|
| No | Yes | Apply remote file or deletion |
| Yes | No | Preserve local file or deletion |
| Same content | Same content | Keep the existing file |
| Yes | Different change | Preserve local work and save the remote file under the receipt |

Agent files use the same content comparison, with one exception. When both copies changed, but each is the sent file with lines appended, the adapter examines the local lines. If they contain no user work, the remote copy replaces the local copy. The undo record keeps the local copy, and `beam down` names the file. For Claude, only session transcripts qualify, and user work is a typed prompt, an image, or a slash command with arguments. Tool results, interruptions, meta lines, and commands without arguments (such as `/exit`) are not. This lets a session that ran `beam` itself come home after its last local reply. Agent-file deletions are not propagated. The adapter declares their return scope. For Claude, it includes the project session directory and file history.

A return package is immutable after successful remote creation. Downloads stream to a private temporary file and rename after success. Extraction streams regular files to disk. Links, traversal, duplicate paths, and unexpected archive entries are rejected.

After a return without conflicts, `beam down` resumes the agent session with a return message as the first prompt. The message tells the agent that it is back on the local machine, whether the sandbox is removed or kept, that sandbox tools and the sandbox Beam commands are not available, and that build results must be checked again. It tells the agent to report the task state and wait for the user. Beam releases the project lock and the tab progress, then replaces its process with the agent in the session directory. It prints the command, with the message, and does not start the agent when there are conflicts, with `--detach`, without a terminal, or when the agent already runs in the project. `beam review --apply` ends the same way.

The `applied` phase is saved before sandbox cleanup. Cleanup retries do not reapply files. A conflict return reports a nonzero exit status after preserving recovery data.

## Setup and readiness

Lockfile detection selects one command per ecosystem. Each rule contains its lockfile, ecosystem, command, and required tool. Explicit setup commands replace detection.

Supported numeric toolchain pins are checked against the destination. Dynamic aliases and unsupported tools produce warnings. Transfer tools and the agent executable remain hard prerequisites. For adapters with `environment_repair`, project tool and version checks run after upload. Missing project tools are repair context for the agent. Shell transfers keep the full preflight requirement. Custom images and Steel checkpoints can supply tools in advance.

In sandboxes that Beam creates (Docker, Steel, and Daytona, not SSH hosts), the remote checks first install missing known tools: Node.js, npm, and npx from the official Linux build after its SHA-256 sum matches; pnpm through npm; cargo and rustc through rustup; and uv through its installer. A project pin selects the version. For Node.js and pnpm without a pin, the version on the local machine applies, then Node.js 22 and the latest pnpm. Where apt-get exists and the checks run as root, Beam also installs `libatomic1` when the installed Node.js does not start, and `gcc libc6-dev` for Rust when no C compiler exists. A failed installation does not stop the checks; the prerequisite check reports the missing tool. An existing tool stays; when an existing Node.js has another major version than the plan, the setup log says so. The plan shows the tools and versions. Other tools stay repair context.

Remote project prerequisites, setup, and verification run in order and stop on the first failure. Setup and verification output appears in the remote terminal while each command runs. Output is saved in a persistent setup log. A repair-capable agent starts even when these checks fail. Its handoff contains the saved prerequisite checks, setup and verification commands, original task context, and the last 80 lines from each attempted check log. Logs are diagnostic data, not instructions.

The launcher exports `BEAM_CHECK`, a generated script containing the saved checks. The agent repairs the environment without weakening checks or project version requirements, then invokes `sh "$BEAM_CHECK"`. The script reruns prerequisites, setup, and verification in the session directory. It clears repair status only on success. Completion reports cannot mark the environment ready. An empty verification list remains explicitly unverified. Check invocations use a separate lock to prevent concurrent setup. A failed check records a `check-failed` event that names the failed step and its last output line. Status shows it as `Failed check` while the environment is in repair or needs attention. Repair turns stay in the resumed session. `beam down` notes that the returned session includes them.

Adapters without `environment_repair` open a manual repair shell and record `needs-attention`. After manual repair, repeating `beam` reruns setup in the same sandbox. Authentication and permissions remain under the agent's normal controls, except for an explicit permission mode. A missing agent executable blocks upload; a running but unauthenticated agent may require user input.

Older saved uploads receive the new launcher after upload and before restore, using the saved launcher variables and prerequisite metadata. Upgraded launchers do not install tools. The workspace snapshot and sandbox identity are preserved. Already-started transfers keep their existing launcher.

Beam prints progress before allocation, remote preparation, upload, restoration, setup, return packing, download, local apply, and cleanup.

The launcher records `preparing`, `repairing`, `needs-attention`, `running`, and `stopped`. Upload waits briefly for readiness. Long setup remains visible through `beam status`, `beam logs`, and `beam attach`.

“Running” describes the remote process. It does not prove authentication, task progress, or task completion. Without task evidence, status reports an unknown task state and recommends `beam status --watch --notify`.

Each agent adapter owns discovery, session files, generated settings, authentication variables, required tools, startup, and observation decoding. When `CLAUDE_CODE_OAUTH_TOKEN` is set, `ANTHROPIC_API_KEY` stays local, because Claude Code prefers the key and asks before it uses it. A variable in `[env] forward` is always sent. The registry currently exposes `claude` and `shell`. Codex is not implemented. Shared transfer and status code use the adapter contract without client-specific paths or terminal patterns.

Capabilities describe session transfer, structured events, terminal heuristics, and agent reports. The Claude adapter supports session transfer and terminal heuristics. It does not yet consume structured client events. The launcher supplies an explicit report command for all agents.

Normalized observations include process start, activity, input needed, input resolved, process exit, completion reported, failure reported, and unknown. Each observation identifies its source. Status keeps process state separate from task state. Within the current process run, the latest observation from each source applies. Structured client events take priority over agent reports, which take priority over terminal heuristics. A new process start clears earlier task reports. Completion reports do not prove correctness.

The Claude fallback checks the final nonempty footer in the visible tmux pane, including a footer wrapped across two lines. It recognizes known English confirmation and permission footers. A match displays `possible-input`, with `input_request.source` set to `terminal-heuristic`. The next action is `beam attach`. Pane content stays remote.

A disappearing footer clears only the heuristic. It does not prove activity or clear a stronger input report. A failed capture supplies an unavailable observation. Unknown dialogs and localized footers remain undetected. Beam never submits a response. It changes permission policy only through an explicit permission mode. Shell sessions have no live probe. Return phases and retained-transfer guidance take priority over task evidence.

The watcher renders and notifies from the same collected snapshot. Repeated observations do not trigger another notification. An unavailable poll does not reset notification history. A confirmed resolution allows a later input request to notify again.

## Providers

| Provider | Allocation | Execution and transfer | Cleanup |
|---|---|---|---|
| Docker | Named and labeled container | Docker command-line tools | Check label, remove container |
| Docker over SSH | Same on the SSH host | SSH plus Docker tools | Same ownership check |
| SSH | Existing host | SSH standard streams | Check directory ownership markers |
| Daytona | New sandbox or named snapshot | API key, curl, OpenSSH | Delete owned sandbox |
| Steel | New computer or checkpoint restore | Steel preview CLI | Delete computer |

Daytona uses its REST API for allocation, state, startup, SSH tokens, and deletion. Commands and binary archives use OpenSSH. The transport preserves remote exit codes and supports interactive terminals. A private temporary header file supplies the API key. Command arguments do not contain the API key.

Allocation syncs its marker file and parent directory before the API request. It saves the response before returning the ID. An ambiguous response requires explicit recovery. Recovery and deletion verify the `beam.session` label.

Beam requests a root-owned sandbox and runs sandbox commands as root. If the SSH gateway uses a different user, Beam requires passwordless sudo and removes the SUDO_* variables, so tools see plain root. Beam installs base tools and the selected agent before upload. Custom targets use `daytona:SNAPSHOT`. The timeout sets an inactivity auto-stop interval, rounded up to minutes. Auto-deletion and the wall-clock time limit are disabled.

Beam starts stopped or archived sandboxes before attachment or return. It waits for snapshot preparation, stopping, archiving, and other temporary states. Startup and deletion polling each have a three-minute timeout. An API request already in progress can extend that interval. Deletion completes only when the sandbox is absent or reports a deleted state. Failed or incomplete cleanup keeps the transfer open for retry. A stopped process requires restart. Paused virtual machines require manual resume.

Steel's command SSH transport does not reliably expose the remote exit code. Beam writes an exit receipt and reads it through the execution API. File downloads stream through SSH. Interactive attachment uses the login-shell hook installed by the bootstrap script. `steel computer ssh` does not stop when the remote shell exits. The attach command writes a done marker. Beam polls for it, stops the client with SIGTERM, and restores the terminal mode.

The project path stays the same on both sides. Docker, Steel, and Daytona use the source HOME path. SSH uses an isolated HOME for agent files.

## Commands in the sandbox

Upload installs a sandbox `beam` command in `.beam/bin` of the stage owner. Docker, Steel, and Daytona sandboxes also get `/usr/local/bin/beam`, but only when that path is free or has an earlier copy of this command. SSH hosts get only `~/.beam/bin/beam`, so a real Beam installation on the host stays unchanged. The launcher puts this directory first on the agent's PATH and exports `BEAM_STAGE`.

The sandbox command has `attach`, `down`, and `ls`. It prints that all other commands run on the local machine. Without an ID, it uses `BEAM_STAGE`, then the only transfer in the sandbox. With several transfers, a terminal shows a numbered choice; otherwise the command asks for an ID.

`beam attach` in the sandbox opens the agent tmux session, or the repair session when only that exists. It sets `window-size latest`, so a second client, such as the Steel web terminal, does not shrink the window to the smaller client. It refuses inside tmux and after a return request.

`beam down` in the sandbox runs `return.sh` from the stage. That script stops the agent and the repair shell like the local return, then runs the same pack script as local `beam down`. It records `return-requested`, then `return-ready` or `return-failed` with a reason. When it runs in the agent or repair terminal, a separate `beam-return-ID` tmux session does the work after 5 seconds. The command returns at once, so the agent can finish its reply. In other terminals, it does the work in the foreground.

The sandbox cannot apply the work. Only local `beam down` applies it, with the usual protection of local work. The return package is immutable, so sandbox edits after the pack do not return. A `pack-lock` directory serializes the sandbox and local packs; the second pack reuses the package. After a return request, local `beam attach` and `beam restart` refuse and recommend `beam down`. Status shows `packing for return`, `packed for return`, or `sandbox pack failed`, before process state. `beam status --watch --notify` notifies for `return-ready` and `return-failed`.

`beam down --wait` polls the sandbox every 5 seconds without the project lock, until the work is packed or the pack failed. It does not wake a paused sandbox. After a failed sandbox pack, the local return tries the pack again. Ten consecutive failed polls stop the wait.

The Claude adapter adds a `beam` skill to the sandbox `~/.claude/skills` as a default file. It tells the agent to run `beam down` only when the user asks for the return, and to use `$BEAM_REPORT` to report completion. The skill does not return with the session. It replaces a local skill with the same path, which upload does not send.

## Retention and removal

`beam down --keep` records `retained`. The sandbox remains listed and can be inspected or removed through Beam. Returning again removes the retained sandbox without importing subsequent edits. Command help, return output, and status explain that further sandbox edits will not return through `beam down`.

`beam kill` removes owned resources without returning additional work. `beam forget --yes` only closes the local record; the user must remove remote resources separately.

Closed receipts retain recovery data. Outgoing archives are removed after the transfer closes. Users can remove recovery worktrees and closed receipt directories after verification. No automatic pruning is implemented. Acknowledging recovery does not prune it.

## Terminal output

Decoration applies only when stdout is a terminal, `NO_COLOR` is unset or empty, and `TERM` is not `dumb`. In all other cases, output is plain text with the same words. JSON output is never decorated.

- Colors come from the Beam palette. Labels, commands, counts, links, and warnings have palette colors. Slow steps show a moving bright band and sparks. Terminals without 24-bit color get the nearest xterm 256 colors. Labels and symbols communicate states without relying on color.
- Slow remote steps show a spinner beside stable text and elapsed time. Animated ASCII descriptions shorten to fit the window without wrapping. Descriptions with non-ASCII text use static output. Completion prints `✓` or `✗` and elapsed time in scrollback. Static output prints a step before work and a result after work.
- `BEAM_ANIMATION=0` disables the spinner, the transfer animation, ambient graphics, shader activation, demo animation, and animated tab indicator. Static colored output and completion notifications remain available. `BEAM_EFFECT=off` disables the transfer animation and ambient effects, and keeps the spinner.
- During the remote part of `beam` (from sandbox creation to agent launch) and of `beam down` (from agent stop until the returned work is saved), the transfer animation from `beam demo` plays in ten rows above the progress line. The steps move it: the workspace leaves with the upload or download and turns as a cloud between the destinations. It forms at the destination during the steps after the transfer: restore and launch for `beam`, and saving the returned work for `beam down`. No step waits for the animation. The destination frames sway, and the frame that holds the formed workspace glows in its color. The cloud fades out before the top and bottom of the animation rows. The cursor is hidden while the animation shows. Other Beam output prints above the animation. The animation is erased on completion, failure, and interruption, so scrollback keeps only the progress lines. It does not use the alternate screen or raw input. It needs at least 40 columns and 20 rows; smaller windows keep the spinner only. A width change draws it again at the new width.
- Ghostty and Kitty can show a faint image beneath the active progress line after 600 ms. The effect preserves scrollback, adds no rows, and ends on completion, interruption, intervening output, or a window too narrow for the line. Multiplexers and unknown terminals get the inline display. Capability detection does not read terminal input.
- The optional `shaders/beam.glsl` shader activates only with `BEAM_EFFECT=shader` in Ghostty. Users install it explicitly. Activation uses a cursor-color marker; cleanup resets the cursor to its theme color. Beam does not install or configure the shader automatically.
- In Ghostty, WezTerm, iTerm2, Windows Terminal, and ConEmu, `beam` and `beam down` show busy progress in the tab (OSC 9;4).
- In terminals that support OSC 8, project, receipt, and worktree paths are clickable links. `FORCE_HYPERLINK=1` enables links in all terminals.
- When `beam` takes 15 seconds or more and the session runs, Beam rings the bell. Ghostty, WezTerm, and iTerm2 also get a desktop notification (OSC 9).
- A successful `beam down` prints one summary line: commits and paths that returned, time away, and conflicts.
- After a successful move, Beam sometimes prints one line of flavor text (1 in 20).
- `beam me up` and `beam me down` are the same as `beam up` and `beam down`. A `scotty` word after them is accepted. Because of this alias, a project directory named `me` must be given as `./me` when it is followed by `up` or `down`.

### Standalone text demo

`beam demo` renders a voxel workspace moving from the local destination to the sandbox destination. `--down` reverses the timeline. The demo needs no Git project, provider, credentials, or graphics protocol. `--seconds` accepts 1 through 30; the default is 6.

Only the explicit demo enters the alternate screen and raw input mode. It hides the cursor and disables wrapping while active. An exit guard restores these settings on completion, handled interruption, error, or unwinding. Esc, q, Q, and end-of-input exit; Ctrl-C and SIGINT interrupt with exit status 130. The handler requests exit; terminal restoration runs on the main thread. Uncatchable termination cannot run the exit guard.

The demo targets about 30 frames per second. Its drawing region is capped at 158 columns and 40 rows. It adapts on resize and avoids the terminal's last column. Below 40 columns or 12 rows, it shows a static preview or a resize hint. The duration continues during resize. Noninteractive input, undecorated output, or disabled animation produces immediate static text without terminal takeover.

`src/scene.rs` computes deterministic geometry from progress, time, and dimensions. `src/raster.rs` rasterizes triangles with reciprocal-depth interpolation, resolves eight dots into each braille cell, averages visible dot colors, and encodes changed runs at absolute positions or relative to the cursor line. Buffers are reused across frames; resizing reallocates them. The renderer owns no terminal input or output. The demo owns timing, bounded input polling, screen control, and output synchronization. `src/show.rs` owns the transfer animation strip. These modules add no dependencies and remain internal.

`beam demo --benchmark` measures software rendering and frame encoding for 180 frames at 80x24, 120x40, and 160x50. It reports median and 95th-percentile frame time, encoded bytes per frame, and estimated output volume at 30 frames per second. These measurements exclude terminal drawing, input, and demo labels.

## Validation

Offline Daytona tests cover allocation markers, interrupted allocation, ownership, snapshot preparation, stop/archive transitions, delayed deletion, and cleanup retry after API failure. The optional Daytona integration test checks a real shell round trip and binary file transfer. Authenticated Claude continuation on Daytona requires a separate smoke test.

Local tests cover snapshot round trips, ownership checks, locking, file merging, archive validation, configuration, and command errors. Docker tests cover normal transfers, setup failure and retry, shell workspaces, returning extras, retained sandboxes, conflict recovery, and retry after local apply.

Steel integration tests allocate a real computer and remove it afterward. They exercise transport and simulated agent changes. Authenticated Claude continuation requires a separate smoke test with valid credentials and a real transcript. Docker tests also cover reviewed returns, preserved staging during merges, extra-file undo, stale plans, recovery acknowledgment, verification failure, task reports, notifications, blocked first-run notices, and setup reuse/invalidation. Adapter tests cover wrapped footers, stale or quoted prompt text, unsupported probes, evidence precedence, and notification transitions. A second test adapter uses its own session layout and event format through discovery, launch, observation, and return.

## Saved return plans and undo

`beam down --review` stops the remote session and downloads the immutable return package. The transfer stays `downloaded`. It saves the remote worktree, a versioned `return-plan.json`, and `return-files.json`. It does not apply Git changes, extras, or agent files. Git objects and private transfer records may change.

The plan records local and remote snapshots, changed paths, conflicts, and a proposed target snapshot when supported. Automatic combination treats each path's index and worktree entries together. Different edits to the same path require manual integration. It supports separate additions, deletions, binary files, symlinks, and executable modes without flattening staging. File/directory collisions are rejected. Combining diverging commits or branches is not implemented.

A proposed combined result has an inspection worktree. Its edits are not applied by `beam review --apply`. `beam review --open` opens the saved remote worktree for manual inspection and integration. `--diff` shows the sent-to-remote worktree diff without external diff programs. `--json` includes the plan, auxiliary files, task record, events, and timings.

The review leads with path counts and the next action. Its preview shows up to ten local and ten remote paths. Conflicts and returning auxiliary files remain visible. `--json` includes every path. A manual-integration plan recommends opening the saved worktree; an applied return explains that `beam undo` can restore the pre-return state.

Before applying, Beam checks the reviewed local Git snapshot and auxiliary file fingerprints. A changed Git snapshot rebuilds the plan and stops. `beam review --refresh` rebuilds an unapplied plan, including auxiliary files. It does not change the fixed remote snapshot. After apply starts, unexpected local changes stop the retry and leave saved copies for recovery.

Before mutation, Beam saves the local Git state in `before-return`, auxiliary file backups, an undo record, and an apply marker. Retries recognize the proposed Git state and already-returned auxiliary files. A partial Git checkout that does not match a known snapshot stops for recovery rather than guessing which changes belong to the user.

`beam undo` checks all recorded auxiliary paths before changing Git. It restores the previous Git state and auxiliary files only when current state matches the return or an already-restored state. Later incompatible edits cause failure and preserve both versions. Undo is repeatable, keeps returned work, and does not recreate or remove a sandbox. It restores Git state and file contents/modes; it does not undo external actions performed by the agent or restore running processes.

Review and undo default to the latest receipt with a saved return plan for the project. `--transfer ID` selects another receipt. Legacy receipts without saved plans remain inspectable through their existing recovery paths.

## Task records, evidence, and reuse

The optional `[task]` section supplies objective, completion criteria, last verified result, next action, and constraints. The preview, saved task record, and handoff retain these user-provided facts. Automatic transcript summarization is not implemented.

`[sandbox] verify` runs shell commands after setup in the session directory. A failed command starts environment repair for capable agents and manual recovery for shell transfers. Authentication and task completion remain unverified by process status.

The launcher appends timestamped events for setup, verification, agent startup, and exit. Command events record exit status and elapsed seconds. The `BEAM_REPORT` script accepts explicit `working`, `waiting`, `finished`, and `failed` reports with bounded single-line evidence. Status labels these as agent reports. Beam does not infer completion from inactivity or successful process exit.

`beam status --watch` polls evidence and prints changed status. `--notify` rings the terminal bell once for each newly observed attention, completion, failure, or exit event during that watch. Existing events do not notify on startup. A live terminal input request does notify on the first observation, even on startup. Repeated observations do not notify again until the prompt kind changes or the prompt clears. Failed polls do not count as clearing a prompt. Polling stops after `--count`, when the transfer closes, or when the user interrupts it. It does not install a background service or send external notifications.

Receipts contain diagnostic timings between saved phases. These include network operations and user waiting time. They are not CPU execution measurements. Return downloads retain the last 100 remote events for later inspection.

Optional `reuse_setup` requires configured verification and applies only within one sandbox. The fingerprint includes launcher content, forwarded environment, current commit and workspace tree, tool locations and version output, and explicit `setup_inputs`. Setup success plus verification success saves a fingerprint of the resulting environment inputs. Checks always rerun. Changed/missing inputs or verification failure disable reuse. Ignored dependencies are validated by the configured checks; ignored setup inputs must be listed explicitly. No artifacts are shared across projects or newly allocated sandboxes.

`beam restart` restarts only a stopped session or one needing attention. It retains the original task and configuration in the launcher. It cannot terminate a running session or restart a returning or retained transfer.

## Future work

These capabilities are not implemented or promised by the current commands:

- Non-Git workspaces, submodules, and separate Git LFS object transfer.
- Other coding agents and additional cloud providers.
- Beam-managed installation of complete project toolchains. Agents can attempt repair using the supplied context.
- Running service migration or live bidirectional synchronization.
- Automatic pruning of old recovery receipts.


## Optional GitHub publication workflow

`--pr` or `[workflow] pr = true` enables agent instructions for Conventional Commits, regular pushes, relevant checks, and a draft pull request. Beam creates the sandbox branch `beam/TRANSFER_ID` without changing the transferred index or worktree. It does not create task commits, push, or open a pull request itself. The handoff excludes unrelated changes and prohibits force-push and merge. When the return applies, the local checkout moves to `beam/TRANSFER_ID`. The branch that was sent stays at its sent commit.

`--github-auth` or `[workflow] github_auth = true` forwards GitHub authentication independently. PR mode implies authentication. The origin must use GitHub.com HTTPS or SSH. Beam selects `GH_TOKEN`, then `GITHUB_TOKEN`, then the local GitHub CLI token. Missing credentials fail before allocation. Values are never printed or placed in receipts or the upload archive. Upload retry resolves credentials again. The private remote environment file contains the token.

The sandbox uses a repository-local GitHub credential helper through `gh`, and a normalized HTTPS origin. Local remote configuration stays unchanged. The bundled image and cloud bootstrap provide `gh`. Existing custom images and SSH hosts use prerequisite checks and the existing agent repair behavior.

Return packing records publication evidence in `publication.txt`: the number of uncommitted paths, whether the remote branch tip equals the current commit, and the pull request URL if available. Probes use time limits and report unknown on unavailable tools or failed network checks. The evidence belongs to the fixed return snapshot and is copied to the local receipt. Evidence never blocks workspace recovery or causes a commit or push. The existing conflict, review, undo, and cleanup rules still apply.

A pushed branch protects committed code only. External workspace checkpoints, expiry scheduling, automatic sandbox replacement, and clone-based upload remain future work.
