#!/bin/bash
# Sync version across Cargo.toml and plugin manifests
# Called by cargo-release as pre-release-hook
set -e

VERSION=$1

if [ -z "$VERSION" ]; then
    echo "Usage: $0 <version>"
    exit 1
fi

echo "Syncing version to $VERSION..."

# Update plugin.json
if [ -f .claude-plugin/plugin.json ]; then
    jq ".version = \"$VERSION\"" .claude-plugin/plugin.json > tmp.json
    mv tmp.json .claude-plugin/plugin.json
    echo "Updated .claude-plugin/plugin.json"
fi

# Update marketplace.json plugin entry version (if exists)
if [ -f .claude-plugin/marketplace.json ]; then
    jq ".plugins[0].version = \"$VERSION\"" .claude-plugin/marketplace.json > tmp.json
    mv tmp.json .claude-plugin/marketplace.json
    echo "Updated .claude-plugin/marketplace.json"
fi

# Stage the changes so cargo-release commits them
git add .claude-plugin/

echo "Version sync complete: $VERSION"
