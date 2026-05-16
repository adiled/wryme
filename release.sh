#!/usr/bin/env bash
#
# writeme release script
#
# Usage: ./release.sh [patch|minor|major|version]
#   patch: 1.0.0 -> 1.0.1 (default)
#   minor: 1.0.0 -> 1.1.0
#   major: 1.0.0 -> 2.0.0
#   version: specific e.g., 2.0.0
#

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'
log_info() { echo -e "${GREEN}[INFO]${NC} $1"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }

VERSION=$(cat VERSION)
TYPE="${1:-patch}"

increment_version() {
    local version=$1
    local type=$2
    IFS='.' read -ra VER <<< "$version"

    case "$type" in
        patch) VER[2]=$((VER[2] + 1)) ;;
        minor) VER[1]=$((VER[1] + 1)); VER[2]=0 ;;
        major) VER[0]=$((VER[0] + 1)); VER[1]=0; VER[2]=0 ;;
    esac

    echo "${VER[0]}.${VER[1]}.${VER[2]}"
}

if [[ "$TYPE" =~ ^(patch|minor|major)$ ]]; then
    NEW_VERSION=$(increment_version "$VERSION" "$TYPE")
    log_info "Releasing writeme v$VERSION -> v$NEW_VERSION ($TYPE bump)"
else
    NEW_VERSION="$TYPE"
    log_info "Releasing writeme v$NEW_VERSION (specific)"
fi

echo "$NEW_VERSION" > VERSION

git add -A
git commit -m "Release v$NEW_VERSION" || true
git tag -d "v$NEW_VERSION" 2>/dev/null || true
git tag "v$NEW_VERSION"

log_info "Tagged v$NEW_VERSION"
log_info "Pushing to remote..."

if [ -z "$(git remote get-url origin 2>/dev/null)" ]; then
    git remote add origin git@github.com:adiled/wryme.git 2>/dev/null || true
fi

git push -u origin main && git push origin "v$NEW_VERSION" || log_warn "Push failed"

log_info "=== Done ==="
