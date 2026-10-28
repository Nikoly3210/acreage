#!/bin/sh
# Fill in the placeholders once the repository exists.
#   scripts/set-owner.sh GITHUB_OWNER [DOMAIN]
# e.g. scripts/set-owner.sh alexdev getacreage.app
set -eu
OWNER=${1:?usage: scripts/set-owner.sh GITHUB_OWNER [DOMAIN]}
DOMAIN=${2:-}
REPO="https://github.com/$OWNER/acreage"
HOME_URL=${DOMAIN:+https://$DOMAIN}
HOME_URL=${HOME_URL:-https://$OWNER.github.io/acreage}
cd "$(dirname "$0")/.."
for f in site/index.html site/privacy.html README.md packaging/aur/acreage/PKGBUILD packaging/aur/acreage-git/PKGBUILD packaging/flatpak/*.yml; do
  [ -f "$f" ] && sed -i "s|REPO_URL|$REPO|g; s|OWNER|$OWNER|g" "$f"
done
old=packaging/io.github.OWNER.Acreage.metainfo.xml
new=packaging/io.github.$OWNER.Acreage.metainfo.xml
if [ -f "$old" ]; then
  sed -i "s|OWNER|$OWNER|g" "$old"
  # AppStream wants a homepage and a bug tracker.
  sed -i "s|  <launchable|  <url type=\"homepage\">$HOME_URL</url>\n  <url type=\"bugtracker\">$REPO/issues</url>\n  <url type=\"vcs-browser\">$REPO</url>\n  <launchable|" "$old"
  mv "$old" "$new"
fi
[ -n "$DOMAIN" ] && printf '%s\n' "$DOMAIN" > site/CNAME
echo "Done. Check with: git diff; appstreamcli validate --no-net $new"
