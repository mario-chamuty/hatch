#!/usr/bin/env sh
# Install hatch from a GitHub Release.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/hatch-dev/hatch/main/install.sh | sh
#
# Optional environment variables:
#   HATCH_VERSION   Tag to install (default: latest). e.g. v0.2.0
#   HATCH_INSTALL   Install prefix (default: $HOME/.hatch). The binary is
#                   placed under "$HATCH_INSTALL/bin/hatch".
#   HATCH_REPO      owner/name on github.com (default: hatch-dev/hatch).
#   HATCH_NO_MODIFY_PATH  Set to skip the PATH-modification suggestion.
#
# Exit codes: 0 ok, non-zero on any failure (download, checksum, extract).

set -eu

REPO="${HATCH_REPO:-hatch-dev/hatch}"
PREFIX="${HATCH_INSTALL:-$HOME/.hatch}"
BIN_DIR="$PREFIX/bin"
TAG="${HATCH_VERSION:-latest}"

err() { printf 'error: %s\n' "$*" >&2; exit 1; }
info() { printf '%s\n' "$*"; }

require() {
  command -v "$1" >/dev/null 2>&1 || err "missing required command: $1"
}

require uname
require mkdir
require chmod
require tar
# At least one of curl/wget; we'll pick whichever exists.

http_get() {
  url="$1"; dest="$2"
  if command -v curl >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -fSL --retry 3 --retry-delay 1 -o "$dest" "$url"
  elif command -v wget >/dev/null 2>&1; then
    wget --https-only --tries=3 -qO "$dest" "$url"
  else
    err "neither curl nor wget is installed"
  fi
}

http_get_stdout() {
  url="$1"
  if command -v curl >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -fSL --retry 3 --retry-delay 1 "$url"
  elif command -v wget >/dev/null 2>&1; then
    wget --https-only --tries=3 -qO- "$url"
  else
    err "neither curl nor wget is installed"
  fi
}

detect_target() {
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$os" in
    Linux)
      case "$arch" in
        x86_64|amd64) printf '%s\n' "x86_64-unknown-linux-musl" ;;
        aarch64|arm64) printf '%s\n' "aarch64-unknown-linux-musl" ;;
        *) err "unsupported linux architecture: $arch" ;;
      esac
      ;;
    Darwin)
      case "$arch" in
        x86_64) printf '%s\n' "x86_64-apple-darwin" ;;
        arm64|aarch64) printf '%s\n' "aarch64-apple-darwin" ;;
        *) err "unsupported macOS architecture: $arch" ;;
      esac
      ;;
    *)
      err "unsupported OS: $os (use install.ps1 on Windows)"
      ;;
  esac
}

resolve_tag() {
  if [ "$TAG" = "latest" ]; then
    api="https://api.github.com/repos/$REPO/releases/latest"
    body="$(http_get_stdout "$api")" || err "failed to query $api"
    # Extract "tag_name": "v..." without a JSON parser dependency.
    tag="$(printf '%s\n' "$body" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1)"
    [ -n "$tag" ] || err "could not determine latest release tag from GitHub API"
    printf '%s\n' "$tag"
  else
    printf '%s\n' "$TAG"
  fi
}

verify_sha256() {
  file="$1"; expected="$2"
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$file" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$file" | awk '{print $1}')"
  else
    err "no sha256sum or shasum available to verify download"
  fi
  if [ "$actual" != "$expected" ]; then
    err "checksum mismatch: expected $expected, got $actual"
  fi
}

main() {
  target="$(detect_target)"
  tag="$(resolve_tag)"
  version="${tag#v}"
  archive="hatch-${version}-${target}.tar.gz"
  base="https://github.com/$REPO/releases/download/$tag"

  info "installing hatch ${tag} (${target}) -> ${BIN_DIR}/hatch"

  tmp="$(mktemp -d 2>/dev/null || mktemp -d -t hatch-install)"
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" EXIT INT HUP TERM

  http_get "$base/$archive" "$tmp/$archive"
  http_get "$base/$archive.sha256" "$tmp/$archive.sha256"

  expected="$(awk '{print $1}' "$tmp/$archive.sha256")"
  [ -n "$expected" ] || err "empty checksum from $archive.sha256"
  verify_sha256 "$tmp/$archive" "$expected"

  tar -C "$tmp" -xzf "$tmp/$archive"
  src="$tmp/hatch-${version}-${target}/hatch"
  [ -f "$src" ] || err "extracted archive missing hatch binary at $src"

  mkdir -p "$BIN_DIR"
  install -m 0755 "$src" "$BIN_DIR/hatch" 2>/dev/null || {
    cp "$src" "$BIN_DIR/hatch"
    chmod 0755 "$BIN_DIR/hatch"
  }

  info "installed: $("$BIN_DIR/hatch" --version 2>/dev/null || printf '%s' "$BIN_DIR/hatch")"

  case ":$PATH:" in
    *":$BIN_DIR:"*)
      ;;
    *)
      if [ -z "${HATCH_NO_MODIFY_PATH:-}" ]; then
        cat <<EOF

$BIN_DIR is not on your PATH. Add it by appending the following line to your
shell profile (e.g. ~/.bashrc, ~/.zshrc, ~/.config/fish/config.fish):

    export PATH="$BIN_DIR:\$PATH"

Then start a new shell, or run:  export PATH="$BIN_DIR:\$PATH"
EOF
      fi
      ;;
  esac
}

main "$@"
