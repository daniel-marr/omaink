#!/usr/bin/env bash
# Per-user install of a release build of OmaInk (no root):
#   ~/.local/bin/omaink
#   ~/.local/share/applications/co.think3.OmaInk.desktop
#   ~/.local/share/icons/hicolor/scalable/apps/co.think3.OmaInk.svg
#
#   packaging/install-local.sh              build + install
#   packaging/install-local.sh --uninstall  remove those three files
#
# Notes, settings and app state are never touched.
set -euo pipefail

APP_ID=co.think3.OmaInk
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_DIR="${HOME}/.local/bin"
DATA_DIR="${XDG_DATA_HOME:-${HOME}/.local/share}"
case "${DATA_DIR}" in /*) ;; *) DATA_DIR="${HOME}/.local/share" ;; esac
APPS_DIR="${DATA_DIR}/applications"
ICON_DIR="${DATA_DIR}/icons/hicolor/scalable/apps"

refresh_caches() {
    command -v update-desktop-database >/dev/null && update-desktop-database -q "${APPS_DIR}" || true
    command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t -f "${DATA_DIR}/icons/hicolor" || true
}

if [[ "${1:-}" == "--uninstall" ]]; then
    rm -f "${BIN_DIR}/omaink" "${APPS_DIR}/${APP_ID}.desktop" "${ICON_DIR}/${APP_ID}.svg"
    refresh_caches
    echo "OmaInk removed (notes, settings and state untouched)."
    exit 0
fi

cargo build --release --manifest-path "${ROOT}/Cargo.toml" -p omaink-gtk
desktop-file-validate "${ROOT}/packaging/${APP_ID}.desktop"

install -Dm755 "${ROOT}/target/release/omaink" "${BIN_DIR}/omaink"
install -Dm644 "${ROOT}/packaging/${APP_ID}.desktop" "${APPS_DIR}/${APP_ID}.desktop"
install -Dm644 "${ROOT}/packaging/icons/hicolor/scalable/apps/${APP_ID}.svg" "${ICON_DIR}/${APP_ID}.svg"
refresh_caches
echo "OmaInk installed: ${BIN_DIR}/omaink"
