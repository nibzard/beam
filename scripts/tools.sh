# ABOUTME: Installs missing project tools that Beam knows how to install: Node.js, npm, pnpm, cargo, and uv.
# ABOUTME: It runs only in sandboxes that Beam creates. Other missing tools stay for the prerequisite check.
# Inputs: TOOLS (one per line), INSTALL_TOOLS (yes|no), NODE_VERSION, PNPM_VERSION, UV_VERSION, RUST_VERSION.
# TOOLS_PREFIX overrides the install prefix (tests).
tools_prefix() {
  if [ -n "${TOOLS_PREFIX:-}" ]; then echo "$TOOLS_PREFIX"
  elif [ -w /usr/local ]; then echo /usr/local
  else echo "$HOME/.local"; fi
}
# Small system libraries, only where apt-get exists and the script runs as root.
apt_install() {
  command -v apt-get >/dev/null 2>&1 && [ "$(id -u)" = 0 ] || return 1
  DEBIAN_FRONTEND=noninteractive apt-get update -qq >/dev/null \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq "$@" >/dev/null
}
# Official Linux build, with its SHA-256 sum from the same release directory.
install_node() {
  command -v node >/dev/null 2>&1 && return 0
  case "$(uname -m)" in
    x86_64|amd64) arch=x64 ;;
    aarch64|arm64) arch=arm64 ;;
    *) echo "beam: no Node.js build for $(uname -m)"; return 1 ;;
  esac
  node_version=${NODE_VERSION:-22}
  case "$node_version" in
    *.*.*) base="https://nodejs.org/dist/v$node_version" ;;
    *) base="https://nodejs.org/dist/latest-v${node_version%%.*}.x" ;;
  esac
  line=$(curl -fsSL "$base/SHASUMS256.txt" | grep " node-v[0-9.]*-linux-$arch.tar.gz\$") || {
    echo "beam: cannot find Node.js $node_version for linux-$arch"; return 1; }
  sum=${line%% *}
  file=${line##* }
  dir=${file%.tar.gz}
  tmp=$(mktemp -d) || return 1
  if curl -fsSL "$base/$file" -o "$tmp/$file" \
    && printf '%s  %s\n' "$sum" "$tmp/$file" | sha256sum -c - >/dev/null 2>&1; then
    prefix=$(tools_prefix)
    mkdir -p "$prefix"
    tar -xzf "$tmp/$file" -C "$prefix" --strip-components=1 --no-same-owner \
      "$dir/bin" "$dir/lib" "$dir/include" "$dir/share"
    code=$?
  else
    echo "beam: the Node.js download failed or its checksum did not match"
    code=1
  fi
  rm -rf "$tmp"
  [ "$code" -eq 0 ] || return "$code"
  # Recent official builds need libatomic, which minimal Debian images do not have.
  if ! "$prefix/bin/node" --version >/dev/null 2>&1; then
    apt_install libatomic1 || true
  fi
  "$prefix/bin/node" --version >/dev/null 2>&1 || { echo "beam: the installed Node.js does not start"; return 1; }
}
install_pnpm() {
  install_node || return 1
  npm install -g --prefix "$(tools_prefix)" "pnpm@${PNPM_VERSION:-latest}"
}
install_cargo() {
  # Most crates need a C linker.
  if ! command -v cc >/dev/null 2>&1; then
    apt_install gcc libc6-dev || echo "beam: no C compiler; crates that need one will not build"
  fi
  set -- -y --profile minimal --no-modify-path --default-toolchain "${RUST_VERSION:-stable}"
  curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs | sh -s -- "$@"
}
install_uv() {
  case "${UV_VERSION:-latest}" in
    latest) url=https://astral.sh/uv/install.sh ;;
    *) url="https://astral.sh/uv/$UV_VERSION/install.sh" ;;
  esac
  curl --proto '=https' --tlsv1.2 -fsSL "$url" | env UV_NO_MODIFY_PATH=1 sh
}
install_tools() {
  [ "$INSTALL_TOOLS" = yes ] || return 0
  # An existing Node.js stays. Say so when its major version differs from the planned one.
  if [ -n "${NODE_VERSION:-}" ] && command -v node >/dev/null 2>&1; then
    have=$(node --version 2>/dev/null | sed 's/^v//')
    if [ "${have%%.*}" != "${NODE_VERSION%%.*}" ]; then
      echo "beam: the sandbox has Node.js $have; the plan asked for $NODE_VERSION. Beam keeps the existing version."
    fi
  fi
  printf '%s\n' "$TOOLS" | while IFS= read -r tool; do
    [ -n "$tool" ] || continue
    command -v "$tool" >/dev/null 2>&1 && continue
    case "$tool" in
      node|npm|npx) recipe=install_node ;;
      pnpm) recipe=install_pnpm ;;
      cargo|rustc) recipe=install_cargo ;;
      uv) recipe=install_uv ;;
      *) continue ;;
    esac
    echo "beam: installing $tool"
    # A failed install is not fatal here: the prerequisite check reports the missing tool.
    "$recipe" || echo "beam: could not install $tool"
  done
}
