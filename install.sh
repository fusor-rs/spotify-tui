#!/bin/sh
set -eu

fail() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

main() {
  [ "$#" -le 1 ] || fail "usage: sh install.sh [VERSION]"
  for tool in curl tar uname; do
    command -v "$tool" >/dev/null 2>&1 || fail "$tool is required to install spotify-tui"
  done
  if command -v sha256sum >/dev/null 2>&1; then
    checksum_command=sha256sum
  elif command -v shasum >/dev/null 2>&1; then
    checksum_command="shasum -a 256"
  else
    fail "sha256sum or shasum is required to verify the download"
  fi

  system=$(uname -s)
  processor=$(uname -m)
  case "$system" in
    Darwin) platform="apple-darwin" ;;
    Linux) platform="unknown-linux-gnu" ;;
    *) fail "unsupported system $system; spotify-tui requires macOS or Linux" ;;
  esac
  case "$processor" in
    x86_64 | amd64) architecture="x86_64" ;;
    arm64 | aarch64) architecture="aarch64" ;;
    *) fail "unsupported processor $processor; use x86-64 or ARM64" ;;
  esac
  target="$architecture-$platform"

  releases="https://github.com/fusor-rs/spotify-tui/releases"
  version=${1:-${SPOTIFY_TUI_VERSION:-}}
  if [ -z "$version" ]; then
    latest=$(curl -fsSL -o /dev/null -w '%{url_effective}' "$releases/latest") ||
      fail "cannot find the latest release; check $releases or specify a version"
    case "$latest" in
      "$releases"/tag/v*) version=${latest##*/} ;;
      *) fail "no release found; check $releases" ;;
    esac
  fi
  version=${version#v}
  case "$version" in
    '' | [!0-9]* | *[!0-9A-Za-z.+-]*)
      fail "invalid version; use a release version such as v0.1.0"
      ;;
  esac

  name="spotify-tui-$version-$target"
  archive="$name.tar.gz"
  download_base=${SPOTIFY_TUI_DOWNLOAD_BASE:-$releases/download}
  bin_directory="${SPOTIFY_TUI_INSTALL:-$HOME/.spotify-tui}/bin"
  destination="$bin_directory/spt"
  [ ! -d "$destination" ] || fail "$destination is a directory; choose another installation path"
  umask 077
  mkdir -p "$bin_directory"
  temporary=$(mktemp -d "$bin_directory/.install.XXXXXX")
  trap 'rm -r "$temporary"' 0
  trap 'exit 1' HUP INT TERM

  url="$download_base/v$version/$archive"
  printf 'Downloading spotify-tui v%s for %s\n' "$version" "$target"
  curl -fsSL "$url" -o "$temporary/$archive" || fail "cannot download $url"
  curl -fsSL "$url.sha256" -o "$temporary/$archive.sha256" ||
    fail "cannot download $url.sha256"
  actual=$($checksum_command "$temporary/$archive")
  expected=$(cut -d ' ' -f 1 "$temporary/$archive.sha256")
  [ "$expected" = "${actual%% *}" ] || fail "checksum mismatch; download the release again"

  tar -xzf "$temporary/$archive" -C "$temporary" "$name/spt"
  replacement="$temporary/$name/spt"
  if [ ! -f "$replacement" ] || [ -L "$replacement" ]; then
    fail "archive has no regular spt executable"
  fi
  chmod 755 "$replacement"
  installed_version=$("$replacement" --version) ||
    fail "downloaded spt could not run; check the runtime requirements in README.md"
  [ "$installed_version" = "spt $version" ] || fail "downloaded spt has the wrong version"
  mv -f "$replacement" "$destination"
  printf 'Installed spotify-tui v%s to %s\n' "$version" "$destination"

  case ":$PATH:" in
    *":$bin_directory:"*) ;;
    *)
      printf '\nAdd this to your shell profile, such as ~/.zshrc or ~/.bashrc:\n'
      printf "  export PATH=\"%s:\$PATH\"\n" "$bin_directory"
      ;;
  esac
}

main "$@"
