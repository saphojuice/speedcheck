#!/bin/sh
# SAPHOJUICE speed check, double-click version for macOS.
#
# Same logic as the one-liner. It runs the canonical script, scripts/check in
# github.com/saphojuice/speedcheck, so there is only one implementation to audit:
# download the probe, verify its SHA-256 against the published release, run it, delete it.
#
# Nothing is installed. Because you downloaded this file, macOS may warn that it came from
# the internet, and Gatekeeper may ask you to confirm. Pasting the one-liner into Terminal
# avoids that entirely:
#     curl -fsSL https://saphojuice.com/check | sh
echo ""
echo "  SAPHOJUICE speed check"
echo "  Nothing installs. The probe is deleted when it finishes."
echo ""
curl -fsSL https://saphojuice.com/check | sh
echo ""
printf "  Press Return to close this window. "
read -r _
