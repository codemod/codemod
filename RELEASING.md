# Releasing the Codemod CLI

Run these commands from `codemod/codemod` with `gh` authenticated and `npm` signed in
as a maintainer authorized to change `codemod` dist-tags. Set `ref` to the pushed
release branch containing these workflows and the exact code you intend to ship.
GitHub requires each manually dispatched workflow to exist on the default branch.
Check the resolved commit before shipment and approve the `production` job if prompted.

```bash
ref='RELEASE_BRANCH'
```

The Cargo Release workflow publishes the seven platform packages **before** the
`codemod` wrapper. It publishes all eight packages at the same version and under
the same tag (`alpha` for an alpha, `next` for 2.0.0, `v1` for tagged 1.18.x
security fixes). The platform packages are:

- `@codemod.com/cli-darwin-arm64`
- `@codemod.com/cli-darwin-x64`
- `@codemod.com/cli-linux-x64-gnu`
- `@codemod.com/cli-linux-arm64-gnu`
- `@codemod.com/cli-linux-x64-musl`
- `@codemod.com/cli-linux-arm64-musl`
- `@codemod.com/cli-win32-x64-msvc`

The wrapper pins its optional platform dependencies to exact versions. Promotion,
rollback, and the initial `v1` tag change **only the wrapper tag**. Leave the
platform tags alone: a wrapper installed via `latest`, `next`, `alpha`, or `v1`
fetches its platform package by version, not tag.

## Publish an alpha

Set `version` to a fresh prerelease. Don't reuse a published version.

```bash
version=2.0.0-alpha.10
gh workflow run cargo-release.yml --ref "$ref" -f version="$version" -f dist-tag=alpha
```

After Cargo Release succeeds, verify `alpha` points to that version and smoke-test it:

```bash
test "$(npm view codemod dist-tags.alpha)" = "$version" &&
  gh workflow run release-smoke.yml --ref "$ref" -f version="$version"
```

## Publish 2.0.0 to `next`

Before starting, record the currently shipped 1.18.x `latest` in the release
tracking issue as the rollback version. Check that repository health and docs
validation pass and review the upgrade notes. Do not use `latest` in Cargo Release.

```bash
npm view codemod dist-tags
version=2.0.0
gh workflow run cargo-release.yml --ref "$ref" -f version="$version" -f dist-tag=next
```

Use only one publishing path. Pushing `codemod-cli@2.0.0` also runs Cargo Release
with `next`, plus the existing GitHub Release jobs. Don't push that tag after
publishing the same version by dispatch; npm won't let the workflow publish it again.

## Soak for at least one week

Start the clock when public npm shows `next` pointing to 2.0.0, after all eight
packages have published. Record the UTC start time and earliest promotion time
(at least seven full days later).

```bash
npm view codemod dist-tags
npm view codemod@2.0.0 optionalDependencies --json
gh workflow run release-smoke.yml --ref "$ref" -f version=2.0.0
```

- Confirm `next` is 2.0.0, `latest` is still the recorded 1.18.x, and all seven
  optional dependencies are pinned to 2.0.0.
- Review all seven smoke jobs. The six Linux and macOS jobs must pass;
  log any Windows failure separately, but don't block on it.
- Try the documented 1.x upgrade and a representative 2.0 workflow on a clean
  install. Check install and runtime reports, support issues, and migration feedback
  for release-blocking regressions. Fix and resoak a replacement if necessary;
  published versions are immutable. Never promote a broken 2.0.0.
- Rerun smoke before promotion. Record the results, the release owner's sign-off,
  and the tested rollback version in the tracking issue.

## Tag the supported 1.x release as `v1`

Do this before moving `latest`, while it still points to the recorded 1.18.x.
Run in Bash and check the version printed before proceeding.

```bash
v1="$(npm view codemod dist-tags.latest)"
printf '1.x / rollback version: %s\n' "$v1"
[[ "$v1" =~ ^1\.18\.[0-9]+$ ]] &&
  npm dist-tag add "codemod@$v1" v1 &&
  test "$(npm view codemod dist-tags.v1)" = "$v1"
```

From the promotion date, ship 1.18.x security fixes for three months. On the 1.x
maintenance branch, carry over the Cargo Release change that routes stable
`codemod-cli@1.18.x` tags to `v1` **before** pushing a security release tag.
A workflow at an older tag commit may still publish to `latest`.

```bash
# Set this to the new, unpublished patch version on the 1.x maintenance branch.
patch='1.18.PATCH'
tag="codemod-cli@$patch"
[[ "$patch" =~ ^1\.18\.[0-9]+$ ]] &&
  git tag "$tag" &&
  git push origin "refs/tags/$tag"
```

After the release, verify `npm view codemod dist-tags` shows the patch under
`v1` and leaves `latest` unchanged. Update the recorded rollback version only
after validating the patch.

## Promote 2.0.0 to `latest`

Once the soak is complete and `v1` is verified, move the wrapper tag. Do not rebuild
or republish. Keep `next` on 2.0.0. Record the promotion date and the end of the
three-month security window.

```bash
npm dist-tag add codemod@2.0.0 latest
test "$(npm view codemod dist-tags.latest)" = 2.0.0
npx --yes --prefer-online codemod@latest --version
```

## Roll back

Set `rollback` to the validated 1.18.x version recorded in the release issue.
Move the wrapper tag back. Do not unpublish or retag the platform packages.

```bash
rollback='1.18.VALIDATED_PATCH'
[[ "$rollback" =~ ^1\.18\.[0-9]+$ ]] &&
  npm dist-tag add "codemod@$rollback" latest &&
  test "$(npm view codemod dist-tags.latest)" = "$rollback" &&
  npx --yes --prefer-online codemod@latest --version
```

Leave `v1` and `next` as-is, and announce the rollback. It affects future
installs via `latest`; existing installs, locks, and explicit pins do not change.
Do not deprecate 1.x while the rollback is active.

## Deprecate 1.x after the security window

Only run this once 2.x is on `latest` and the three-month security window has
ended. Replace `<YYYY-MM-DD>` with the end-of-support date. Deprecation warns on
install; it does not remove versions or the `v1` tag.

```bash
npm deprecate 'codemod@1.x' 'Codemod 1.x reached end of support on <YYYY-MM-DD> and no longer receives security fixes. Upgrade to Codemod 2.0 or later: npm install -g codemod@latest.'
```
