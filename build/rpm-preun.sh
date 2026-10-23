#!/bin/bash
# rpm passes the number of versions left after the transaction: 0 on removal,
# 1 or more on an upgrade. On an upgrade the old package's %preun runs AFTER the
# new package's %post, so cleaning up then would delete what the new version has
# just installed (the polkit policy, the SELinux labels).
if [ "${1:-0}" -gt 0 ] 2>/dev/null; then
  exit 0
fi
# Remove polkit policy on uninstall
rm -f /usr/share/polkit-1/actions/com.legiz-ru.prizrak-box.policy 2>/dev/null || true
# Drop the SELinux file-context rules added for the service binaries (rpm-post.sh).
if command -v semanage >/dev/null 2>&1; then
  for f in /usr/lib/Prizrak-Box/resources/px /usr/lib/Prizrak-Box/resources/px-service; do
    semanage fcontext -d "$f" 2>/dev/null || true
  done
fi
