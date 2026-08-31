#!/usr/bin/env bash
#
# Cuts a release: bumps every version in the tree, refreshes Cargo.lock,
# commits, waits for CI, then pushes the `v*` tag that
# `.github/workflows/release.yml` builds binaries from.
#
#   scripts/release.sh --check            verify versions agree; writes nothing
#   scripts/release.sh patch              0.0.1 -> 0.0.2
#   scripts/release.sh minor              0.0.1 -> 0.1.0
#   scripts/release.sh major              0.0.1 -> 1.0.0
#   scripts/release.sh 0.4.0-rc.1         an exact version
#
#   --dry-run      do everything except commit, tag and push; restores the tree
#   --no-ci-wait   tag without waiting for the CI run to go green
#   --yes          skip the confirmation prompt (for non-interactive use)
#
# Why a script rather than a checklist. The version lives in 42 places: one
# `[workspace.package]` line, 40 path-dependency pins in the root
# `Cargo.toml`, and `packages/web/package.json`. Missing one of them either
# fails the build or ships a binary whose `--version` lies. And the release
# workflow builds with `--locked`, so `Cargo.lock` must already carry the
# new version at the tagged commit — a step that is invisible until the
# release job fails.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly CARGO_TOML="Cargo.toml"
readonly WEB_PKG="packages/web/package.json"
readonly RELEASE_BRANCH="main"

die() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }
info() { printf '\033[36m•\033[0m %s\n' "$*"; }
ok() { printf '\033[32m✓\033[0m %s\n' "$*"; }

# --- version helpers -------------------------------------------------------

# The workspace version is the single source of truth every other version in
# the tree is checked against.
current_version() {
	# `...` rather than `..`: the two-dot flip-flop tests its right operand
	# on the same line the left one matched, and `[workspace.package]` is
	# itself a `^\[` line, so the range would close before reading anything.
	perl -ne 'if (/^\[workspace\.package\]/ ... /^\[/) { print "$1\n" and exit if /^version = "([^"]+)"/ }' "$CARGO_TOML"
}

# Accepts `X.Y.Z` with an optional `-prerelease` suffix. Build metadata
# (`+meta`) is rejected: it is legal semver but `+` is not valid in a git
# ref name, so a tag could not be created from it anyway.
validate_version() {
	[[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] \
		|| die "'$1' is not a version this script can tag (expected X.Y.Z or X.Y.Z-pre)"
}

bump_version() {
	local current="$1" kind="$2" core major minor patch
	core="${current%%-*}"
	IFS=. read -r major minor patch <<<"$core"
	case "$kind" in
		major) printf '%d.0.0\n' "$((major + 1))" ;;
		minor) printf '%d.%d.0\n' "$major" "$((minor + 1))" ;;
		patch) printf '%d.%d.%d\n' "$major" "$minor" "$((patch + 1))" ;;
		*) die "unknown bump kind '$kind'" ;;
	esac
}

# Rewrites the workspace version and every path-dependency pin that carries
# it. Two distinct line shapes, matched separately rather than by a blanket
# search-and-replace, so an unrelated third-party dependency that happens to
# sit at the same version is never touched.
write_version() {
	local old="$1" new="$2"
	OLD="$old" NEW="$new" perl -pi -e '
		if (/^version = "\Q$ENV{OLD}\E"\s*$/) {
			$_ = qq{version = "$ENV{NEW}"\n};
		} elsif (/path\s*=\s*"/) {
			s/version\s*=\s*"\Q$ENV{OLD}\E"/version = "$ENV{NEW}"/g;
		}
	' "$CARGO_TOML"

	# Only the top-level `"version"` key; dependency version ranges are
	# values, never this key, so the first match is the right one.
	OLD="$old" NEW="$new" perl -pi -e '
		if (!$done && s/^(\s*"version":\s*)"\Q$ENV{OLD}\E"/$1"$ENV{NEW}"/) { $done = 1 }
	' "$WEB_PKG"
}

# Every place a version is written must agree with the workspace version.
# Run on its own via `--check`, and again after a bump as the proof that the
# rewrite above reached everything.
check_versions() {
	local version stale web
	version="$(current_version)"
	[[ -n "$version" ]] || die "could not read [workspace.package] version from $CARGO_TOML"

	stale="$(grep -n 'version = "' "$CARGO_TOML" | grep -v "version = \"$version\"" | grep 'path = ' || true)"
	[[ -z "$stale" ]] || die "path dependencies disagree with $version:"$'\n'"$stale"

	web="$(perl -ne 'print "$1" and exit if /^\s*"version":\s*"([^"]+)"/' "$WEB_PKG")"
	[[ "$web" == "$version" ]] || die "$WEB_PKG is $web but the workspace is $version"

	printf '%s\n' "$version"
}

# --- preflight -------------------------------------------------------------

preflight() {
	local branch
	command -v cargo >/dev/null || die "cargo not found"
	command -v perl >/dev/null || die "perl not found"

	branch="$(git rev-parse --abbrev-ref HEAD)"
	[[ "$branch" == "$RELEASE_BRANCH" ]] \
		|| die "releases are cut from '$RELEASE_BRANCH', not '$branch'"

	[[ -z "$(git status --porcelain)" ]] \
		|| die "working tree is dirty — commit or stash first"

	info "fetching origin"
	git fetch --quiet origin "$RELEASE_BRANCH" --tags

	[[ "$(git rev-parse HEAD)" == "$(git rev-parse "origin/$RELEASE_BRANCH")" ]] \
		|| die "local $RELEASE_BRANCH is not level with origin/$RELEASE_BRANCH — pull or push first"
}

# --- main ------------------------------------------------------------------

DRY_RUN=false
WAIT_CI=true
ASSUME_YES=false
TARGET=""

while [[ $# -gt 0 ]]; do
	case "$1" in
		--check)
			# The version goes to stdout and the human line to stderr, so
			# `version="$(scripts/release.sh --check)"` works. CI uses that
			# to compare a tag against the tree, which keeps one parser for
			# both rather than a second one in YAML that can drift.
			version="$(check_versions)"
			printf '%s\n' "$version"
			printf '\033[32m✓\033[0m every version in the tree agrees: %s\n' "$version" >&2
			exit 0
			;;
		--dry-run) DRY_RUN=true ;;
		--no-ci-wait) WAIT_CI=false ;;
		--yes|-y) ASSUME_YES=true ;;
		-h|--help) sed -n '3,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
		-*) die "unknown flag '$1'" ;;
		*) [[ -z "$TARGET" ]] || die "give exactly one version or bump kind"; TARGET="$1" ;;
	esac
	shift
done

[[ -n "$TARGET" ]] || die "usage: scripts/release.sh <patch|minor|major|X.Y.Z> [--dry-run] [--no-ci-wait] [--yes]
       scripts/release.sh --check"

preflight
OLD_VERSION="$(check_versions)"

case "$TARGET" in
	major|minor|patch) NEW_VERSION="$(bump_version "$OLD_VERSION" "$TARGET")" ;;
	*) NEW_VERSION="$TARGET" ;;
esac
validate_version "$NEW_VERSION"

readonly TAG="v$NEW_VERSION"
[[ "$NEW_VERSION" != "$OLD_VERSION" ]] || die "already at $OLD_VERSION"
git rev-parse -q --verify "refs/tags/$TAG" >/dev/null && die "tag $TAG already exists locally"
git ls-remote --exit-code --tags origin "refs/tags/$TAG" >/dev/null 2>&1 \
	&& die "tag $TAG already exists on origin"

info "$OLD_VERSION -> $NEW_VERSION  (tag $TAG)"

# `--workspace` rewrites only this workspace's own members in Cargo.lock —
# it does not touch third-party dependency resolution, so the lockfile the
# release builds `--locked` against changes by exactly the versions above
# and nothing else.
info "refreshing Cargo.lock"
write_version "$OLD_VERSION" "$NEW_VERSION"
cargo update --workspace --quiet

check_versions >/dev/null
ok "every version in the tree now reads $NEW_VERSION"

printf '\n'
git --no-pager diff --stat
printf '\n'

if $DRY_RUN; then
	git checkout -- "$CARGO_TOML" "$WEB_PKG" Cargo.lock
	ok "dry run — tree restored, nothing committed, tagged or pushed"
	exit 0
fi

if ! $ASSUME_YES; then
	printf 'Commit, push %s, and publish a GitHub Release? [y/N] ' "$TAG"
	read -r reply
	[[ "$reply" == [yY] ]] || { git checkout -- "$CARGO_TOML" "$WEB_PKG" Cargo.lock; die "aborted"; }
fi

git add "$CARGO_TOML" "$WEB_PKG" Cargo.lock
git commit --quiet --message "chore(release): $TAG"
git push --quiet origin "$RELEASE_BRANCH"
ok "pushed the version bump to $RELEASE_BRANCH"

# The tag is what publishes binaries, so it goes last and only over a commit
# CI has already accepted. Tagging first and asking questions later is how a
# broken release reaches the Releases page.
if $WAIT_CI; then
	if command -v gh >/dev/null; then
		info "waiting for CI on $(git rev-parse --short HEAD)"
		sleep 5
		run_id="$(gh run list --branch "$RELEASE_BRANCH" --commit "$(git rev-parse HEAD)" \
			--limit 1 --json databaseId --jq '.[0].databaseId' 2>/dev/null || true)"
		if [[ -n "$run_id" ]]; then
			gh run watch "$run_id" --exit-status \
				|| die "CI failed — the bump is pushed but no tag was created. Fix, then re-run with an exact version."
			ok "CI is green"
		else
			info "no CI run found yet for this commit; skipping the wait"
		fi
	else
		info "gh not installed; skipping the CI wait"
	fi
fi

git tag --annotate "$TAG" --message "$TAG"
git push --quiet origin "$TAG"
ok "pushed $TAG"

printf '\n'
info "the release workflow builds four targets; follow it at:"
printf '    https://github.com/SenkenGuard/senken/actions/workflows/release.yml\n'
