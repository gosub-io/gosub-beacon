#!/usr/bin/env bash
#
# One-time setup: put a Developer ID certificate where package.sh can reach it, and write the
# configuration package.sh reads.
#
#     ./packaging/setup-signing.sh ~/DeveloperID.p12
#
# The certificate goes into its own keychain rather than the login one, because signing here
# happens over ssh. Nothing logged into this Mac graphically, so the login keychain is locked,
# and `codesign` against a locked keychain does not fail -- it waits forever on a dialog that
# no one can see. A dedicated keychain can be unlocked from a script.
#
# Run once per machine. package.sh unlocks the keychain itself on every build after that.
#
# Making the .p12 on the machine that holds the private key:
#
#     openssl x509 -inform DER -in developerID_application.cer -out cert.pem
#     openssl x509 -inform DER -in <the issuing intermediate>.cer -out intermediate.pem
#     openssl pkcs12 -export -inkey developerID.key -in cert.pem -certfile intermediate.pem \
#       -name "Developer ID Application" -out DeveloperID.p12 \
#       -keypbe PBE-SHA1-3DES -certpbe PBE-SHA1-3DES -macalg sha1
#
# Two details there are easy to get wrong and hard to diagnose from the error:
#
#   The intermediate has to be the one that actually issued the certificate. Apple runs two
#   Developer ID sub-CAs, and the certificate names its issuer by key id -- compare the leaf's
#   Authority Key Identifier with each candidate's Subject Key Identifier rather than guessing.
#   The wrong one imports without complaint and then reports CSSMERR_TP_NOT_TRUSTED.
#
#   The three algorithm flags are not optional. OpenSSL 3 defaults to a SHA-256 MAC, which
#   Apple's Security framework cannot verify and reports as "MAC verification failed ... wrong
#   password?" -- so a correct password looks wrong.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
keychain="$HOME/Library/Keychains/beacon.keychain-db"
env_file="$here/signing.env"

p12="${1:-}"
if [[ -z "$p12" || ! -f "$p12" ]]; then
    echo "usage: $(basename "$0") <path to DeveloperID.p12>" >&2
    exit 1
fi

say() { printf '\033[1m==>\033[0m %s\n' "$1"; }

# Read both passwords up front, so nothing half-runs waiting for input. The keychain password
# is one you invent here; it protects this keychain and nothing else.
read -r -s -p "password of $(basename "$p12"): " p12_password; echo
if [[ -f "$keychain" ]]; then
    read -r -s -p "password of the existing beacon keychain: " keychain_password; echo
else
    read -r -s -p "a new password for the beacon keychain: " keychain_password; echo
    read -r -s -p "again: " confirm; echo
    [[ "$keychain_password" == "$confirm" ]] || { echo "they differ" >&2; exit 1; }
fi

# ── keychain ──────────────────────────────────────────────────────────────────

if [[ ! -f "$keychain" ]]; then
    say "creating $keychain"
    security create-keychain -p "$keychain_password" "$keychain"
fi

# Unlocking comes first: changing a locked keychain's settings is itself something macOS
# wants to ask about, and asking is what cannot happen here.
security unlock-keychain -p "$keychain_password" "$keychain"

# A keychain locks itself after five idle minutes by default, and on sleep. Both turn a later
# build into that same invisible-dialog hang, so push the timeout out to six hours.
security set-keychain-settings -lut 21600 "$keychain"

say "importing the certificate"
security import "$p12" -k "$keychain" -P "$p12_password" \
    -T /usr/bin/codesign -T /usr/bin/security

# Without this, every codesign run stops on a "wants to use your confidential information"
# prompt. Over ssh there is nothing to click, so it simply never returns.
security set-key-partition-list \
    -S apple-tool:,apple:,codesign: -s -k "$keychain_password" "$keychain" >/dev/null

# `-s` replaces the search list rather than adding to it, so name everything that should be
# in it. Leave the login keychain out and unrelated tools start failing.
security list-keychains -d user -s "$keychain" "$HOME/Library/Keychains/login.keychain-db"

# ── check what actually landed ────────────────────────────────────────────────
#
# An import can succeed and still leave nothing usable: a development certificate instead of
# a Developer ID one, or a chain missing its intermediate, both show up here as zero valid
# identities rather than as an error.

identity="$(security find-identity -v -p codesigning "$keychain" \
    | sed -n 's/.*"\(Developer ID Application: [^"]*\)".*/\1/p' | head -1)"

if [[ -z "$identity" ]]; then
    echo >&2
    echo "error: no valid Developer ID Application identity in $keychain" >&2
    echo >&2
    security find-identity -p codesigning "$keychain" >&2
    echo >&2
    echo "CSSMERR_TP_NOT_TRUSTED means the chain is incomplete -- rebuild the .p12 with" >&2
    echo "-certfile intermediate.pem. A 'Mac Developer' or 'Apple Development' name means" >&2
    echo "the wrong certificate type was issued: the portal's Developer ID Application is" >&2
    echo "the only one that can be distributed and notarized." >&2
    exit 1
fi

say "identity: $identity"

# ── configuration ─────────────────────────────────────────────────────────────
#
# package.sh sources this. It holds the keychain password, so it stays out of git and out of
# other users' reach.

umask 077
cat > "$env_file" <<ENV
# Written by packaging/setup-signing.sh. Not in git: it holds a password.
#
# package.sh signs with a Developer ID when BEACON_SIGN_IDENTITY is set, and notarizes and
# staples as well once the notary credentials below are filled in. Comment the identity out
# for an ad-hoc demo build.

export BEACON_SIGN_IDENTITY="$identity"
export BEACON_KEYCHAIN="$keychain"
export BEACON_KEYCHAIN_PASSWORD="$keychain_password"

# App Store Connect API key, from Users and Access -> Integrations -> Keys. The .p8 is
# downloadable exactly once, so keep it somewhere you will not lose.
export BEACON_NOTARY_KEY="\${BEACON_NOTARY_KEY:-$HOME/private_keys/AuthKey_XXXXXXXXXX.p8}"
export BEACON_NOTARY_KEY_ID="\${BEACON_NOTARY_KEY_ID:-XXXXXXXXXX}"
export BEACON_NOTARY_ISSUER="\${BEACON_NOTARY_ISSUER:-00000000-0000-0000-0000-000000000000}"
ENV

say "wrote $env_file"
echo
echo "Fill in the three BEACON_NOTARY_* values and ./package.sh signs, notarizes and staples."
echo "Leave them as they are and it signs only -- notarization is skipped with a note."
