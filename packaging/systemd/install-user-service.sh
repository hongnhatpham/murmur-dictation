#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
service_dir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
config_dir="${XDG_CONFIG_HOME:-$HOME/.config}/murmur"

if [[ $# -gt 0 ]]; then
  murmur_bin="$1"
elif command -v murmur >/dev/null 2>&1; then
  murmur_bin="$(command -v murmur)"
else
  murmur_bin="$project_dir/.venv/bin/murmur"
fi
murmur_bin_exec="${murmur_bin//\\/\\\\}"
murmur_bin_exec="${murmur_bin_exec//\"/\\\"}"
murmur_bin_exec="${murmur_bin_exec//&/\\&}"

mkdir -p "$service_dir" "$config_dir"
for name in murmur-dictate.service murmur.service; do
  sed \
    -e "s#@PROJECT_DIR@#$project_dir#g" \
    -e "s#@MURMUR_BIN@#$murmur_bin_exec#g" \
    "$project_dir/packaging/systemd/$name.in" > "$service_dir/$name"
done

cat <<MSG
Installed:
  $service_dir/murmur-dictate.service  # current one-shot CLI pipeline
  $service_dir/murmur.service          # warmed background processor

Next commands:
  systemctl --user daemon-reload
  systemctl --user import-environment WAYLAND_DISPLAY XDG_CURRENT_DESKTOP DBUS_SESSION_BUS_ADDRESS PATH
  systemctl --user enable --now murmur.service
  journalctl --user -u murmur.service -f

For one-shot fixed-duration testing, start murmur-dictate.service manually.
MSG
