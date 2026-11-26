#!/usr/bin/env bash
set -euo pipefail

if [[ ! "$RELEASE_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
  echo "::error::Use an exact release version, not an npm tag or range"
  exit 1
fi

platform=$(node -e '
  const abi = process.platform === "linux"
    ? (process.report.getReport().header.glibcVersionRuntime ? "-gnu" : "-musl")
    : process.platform === "win32" ? "-msvc" : "";
  console.log(`${process.platform}-${process.arch}${abi}`);
')
if [[ "$platform" != "$EXPECTED_PLATFORM" ]]; then
  echo "::error::Expected $EXPECTED_PLATFORM but Node reports $platform"
  exit 1
fi

start=$PWD
scratch=$(mktemp -d)
trap 'cd "$start"; rm -rf "$scratch"' EXIT
mkdir -p "$scratch/home/appdata" "$scratch/home/localappdata" "$scratch/tmp" "$scratch/repo"
touch "$scratch/home/user.npmrc" "$scratch/home/global.npmrc"

# Git Bash paths must be native for npm and the CLI on Windows.
native_path() {
  if command -v cygpath >/dev/null 2>&1; then
    cygpath -am "$1"
  else
    printf '%s\n' "$1"
  fi
}
export HOME="$(native_path "$scratch/home")"
export USERPROFILE="$HOME" APPDATA="$HOME/appdata" LOCALAPPDATA="$HOME/localappdata"
export XDG_CONFIG_HOME="$HOME/config" XDG_CACHE_HOME="$HOME/cache" XDG_DATA_HOME="$HOME/data"
export XDG_STATE_HOME="$HOME/state" GIT_CONFIG_NOSYSTEM=1
export TMPDIR="$(native_path "$scratch/tmp")"
export TMP="$TMPDIR" TEMP="$TMPDIR"
export npm_config_cache="$HOME/npm-cache" npm_config_registry=https://registry.npmjs.org/
export npm_config_userconfig="$HOME/user.npmrc" npm_config_globalconfig="$HOME/global.npmrc"
export npm_config_include=optional DO_NOT_TRACK=1

cd "$scratch/repo"
version=$(npx --yes --prefer-online "codemod@$RELEASE_VERSION" --version)
if [[ "${version%$'\r'}" != "codemod $RELEASE_VERSION" ]]; then
  echo "::error::Expected codemod $RELEASE_VERSION, got $version"
  exit 1
fi
printf '%s\n' "$version"

# Keep the workflow outside the target, and commit the input so the target
# is clean when the workflow writes. No registry workflow or shell step.
cat > ../workflow.ts <<'TS'
import { forEachFile, step } from "codemod/workflow";

export default async function main() {
  await step("Update fixture", forEachFile("message.txt", (file) => {
    file.write(file.text.replace("before", "after"));
  }));
}
TS
printf 'before\n' > message.txt
git init --quiet
git -c core.autocrlf=false add message.txt
git -c user.name='Release Smoke' -c user.email='release-smoke@example.invalid' commit --quiet -m 'Add fixture'
npx --yes "codemod@$RELEASE_VERSION" workflow run -w ../workflow.ts --target . --no-interactive
if [[ "$(cat message.txt)" != after ]]; then
  echo "::error::Smoke workflow did not update the fixture"
  exit 1
fi
