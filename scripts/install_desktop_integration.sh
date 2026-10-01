#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Register .faris study files with the desktop, for the current user only.
#
#   install_desktop_integration.sh /absolute/path/to/faris-app
#   install_desktop_integration.sh --uninstall
#
# Installs, under ${XDG_DATA_HOME:-~/.local/share}:
#   mime/packages/avila-labs-faris-study.xml   MIME type application/vnd.avila-labs.faris-study
#   applications/faris.desktop                 "Open with FARIS" entry running <app> %f
# then refreshes the MIME and desktop databases. Nothing outside that
# directory is touched; --uninstall removes exactly these two files.
set -euo pipefail

data_home="${XDG_DATA_HOME:-$HOME/.local/share}"
mime_xml="$data_home/mime/packages/avila-labs-faris-study.xml"
desktop_file="$data_home/applications/faris.desktop"

refresh() {
    if command -v update-mime-database >/dev/null; then
        update-mime-database "$data_home/mime"
    else
        echo "update-mime-database not found; install shared-mime-info and rerun" >&2
    fi
    if command -v update-desktop-database >/dev/null; then
        update-desktop-database "$data_home/applications"
    else
        echo "update-desktop-database not found (desktop-file-utils); the entry still works in most desktops" >&2
    fi
}

if [ "${1:-}" = "--uninstall" ]; then
    rm -f -- "$mime_xml" "$desktop_file"
    refresh
    echo "Removed $mime_xml and $desktop_file"
    exit 0
fi

if [ "$#" -ne 1 ] || [ "${1#/}" = "$1" ]; then
    echo "usage: $0 /absolute/path/to/faris-app | --uninstall" >&2
    exit 2
fi
app="$1"
if [ ! -x "$app" ]; then
    echo "$app is not an executable file" >&2
    exit 2
fi
# The Exec line is split on spaces and quotes by desktop launchers.
case "$app" in
    *[[:space:]\"\'\\\$\`]*)
        echo "the path must not contain spaces, quotes, backslashes or shell characters: $app" >&2
        exit 2
        ;;
esac

mkdir -p "$(dirname "$mime_xml")" "$(dirname "$desktop_file")"

cat >"$mime_xml" <<'XML'
<?xml version="1.0" encoding="UTF-8"?>
<mime-info xmlns="http://www.freedesktop.org/standards/shared-mime-info">
  <mime-type type="application/vnd.avila-labs.faris-study">
    <comment>FARIS study</comment>
    <comment xml:lang="en">FARIS study</comment>
    <!-- A zip container whose first entry, "mimetype", is stored uncompressed:
         30 bytes of local header, then the name and the type string. -->
    <magic priority="60">
      <match type="string" offset="30" value="mimetypeapplication/vnd.avila-labs.faris-study"/>
    </magic>
    <glob pattern="*.faris"/>
    <sub-class-of type="application/zip"/>
  </mime-type>
</mime-info>
XML

cat >"$desktop_file" <<DESKTOP
[Desktop Entry]
Type=Application
Name=FARIS
GenericName=Fusion blanket study workspace
Comment=Open and save FARIS study files
Exec=$app %f
Terminal=false
Categories=Science;Engineering;
MimeType=application/vnd.avila-labs.faris-study;
DESKTOP

refresh
echo "Installed $mime_xml and $desktop_file"
