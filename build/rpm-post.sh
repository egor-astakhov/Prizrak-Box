#!/bin/bash
# Install polkit policy for px-service elevation
install -m 644 /usr/lib/Prizrak-Box/resources/prizrak-box-service.policy \
  /usr/share/polkit-1/actions/com.legiz-ru.prizrak-box.policy 2>/dev/null || true
# SELinux (Fedora/RHEL): files under /usr/lib are labelled lib_t, so systemd runs
# the TUN service — and the px it spawns — in its own init_t domain, which cannot
# write the user's data directory: px dies on start and the app never opens.
# As bin_t they run in unconfined_service_t like any other third-party service.
# px-service labels itself and px as bin_t when the service is installed
# (src-service/selinux_linux.go); this repeats it on every install/upgrade so
# freshly unpacked binaries keep the label even where only chcon was available.
if command -v selinuxenabled >/dev/null 2>&1 && selinuxenabled; then
  for f in /usr/lib/Prizrak-Box/resources/px /usr/lib/Prizrak-Box/resources/px-service; do
    [ -e "$f" ] || continue
    if command -v semanage >/dev/null 2>&1 && \
       { semanage fcontext -a -t bin_t "$f" 2>/dev/null || semanage fcontext -m -t bin_t "$f" 2>/dev/null; }; then
      restorecon "$f" 2>/dev/null || true
    else
      chcon -t bin_t "$f" 2>/dev/null || true
    fi
  done
fi
# A service that is already running keeps the old binary and, after an upgrade
# from a build without the labels above, the wrong SELinux domain (init_t, which
# cannot write the user's data directory, so px dies on start) until restarted.
if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
  systemctl try-restart PrizrakBoxService.service 2>/dev/null || true
fi
# Kill any running Prizrak-Box instances
pkill -x "Prizrak-Box" 2>/dev/null || true
sleep 1
# Clean up stale Chromium singleton lock files for all users
for user_home in /home/*/; do
  rm -f "$user_home/.config/Prizrak-Box/Singleton"* 2>/dev/null || true
done
rm -f /root/.config/Prizrak-Box/Singleton* 2>/dev/null || true
