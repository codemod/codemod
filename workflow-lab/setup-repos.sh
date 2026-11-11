#!/usr/bin/env bash
# Shallow, read-only clones the lab workflows' default targets point at.
set -euo pipefail
root="${1:-$HOME/lab-repos}"
mkdir -p "$root/react-apps"
for repo in umami-software/umami excalidraw/excalidraw mfts/papermark formbricks/formbricks; do
  dir="$root/react-apps/${repo#*/}"
  [ -d "$dir" ] || git clone -q --depth 1 "https://github.com/$repo" "$dir"
done
if [ ! -d "$root/backstage-src" ]; then
  git clone -q --depth 1 --filter=blob:none --sparse https://github.com/backstage/backstage "$root/backstage-src"
  git -C "$root/backstage-src" sparse-checkout set packages plugins/catalog plugins/catalog-react \
    plugins/techdocs plugins/scaffolder plugins/search plugins/api-docs plugins/org plugins/home
fi
echo "repos in $root"
