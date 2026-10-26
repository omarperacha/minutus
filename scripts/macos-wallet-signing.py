#!/usr/bin/env python3
"""Sign/verify Minutus's private wallet identity without changing Keychain policy.

Only certificate/profile metadata is inspected. Private signing keys stay in
macOS Keychain; codesign owns their authorization. No wallet material is read.
"""

import argparse
import datetime
import hashlib
import os
from pathlib import Path
import plistlib
import re
import subprocess
import sys
import tempfile

REPOSITORY = Path(__file__).resolve().parent.parent
BUNDLE_ID = "com.omarperacha.zeta"
STAGING_BUNDLE_ID = "com.omarperacha.zeta.staging"
APPLICATION_ID = "com.apple.application-identifier"
TEAM_ID = "com.apple.developer.team-identifier"
# Apple Root CA SHA-256, published in Apple's macOS 26 trusted-root list.
# Fail closed when Apple moves provisioning profiles to another root so the
# trust decision receives explicit review instead of accepting CMS contents.
APPLE_PROFILE_ROOT_SHA256 = {
    "B0B1730ECBC7FF4505142C49F1295E6EDA6BCAED7E2C68C5BE91B5A11001F024",
}


def capture(*command):
    return subprocess.run(command, check=True, capture_output=True).stdout


def decode_profile(profile_path):
    """Verify and decode a profile, supporting Apple's current CMS encoding."""
    try:
        return capture("security", "cms", "-D", "-i", str(profile_path))
    except subprocess.CalledProcessError:
        # Some current macOS development profiles fail Security.framework's CMS
        # decoder. OpenSSL is an acceptable fallback only when the complete CMS
        # signer chain validates against an explicitly reviewed Apple root.
        with tempfile.TemporaryDirectory(prefix="zeta-profile-validation-") as temporary:
            certificate_chain = capture(
                "openssl",
                "pkcs7",
                "-inform",
                "DER",
                "-in",
                str(profile_path),
                "-print_certs",
            )
            certificates = re.findall(
                rb"-----BEGIN CERTIFICATE-----.*?-----END CERTIFICATE-----",
                certificate_chain,
                flags=re.DOTALL,
            )
            trusted_root = None
            for certificate in certificates:
                der = subprocess.run(
                    ["openssl", "x509", "-outform", "DER"],
                    input=certificate,
                    check=True,
                    capture_output=True,
                ).stdout
                if hashlib.sha256(der).hexdigest().upper() in APPLE_PROFILE_ROOT_SHA256:
                    trusted_root = certificate
                    break
            if trusted_root is None:
                raise ValueError("The provisioning profile does not chain to a reviewed Apple root.")
            root = Path(temporary) / "AppleRootCA.pem"
            root.write_bytes(trusted_root)
            return capture(
                "openssl",
                "cms",
                "-verify",
                "-verify_retcode",
                "-purpose",
                "any",
                "-inform",
                "DER",
                "-in",
                str(profile_path),
                "-CAfile",
                str(root),
            )


def profile_identity(profile, now, bundle_id=BUNDLE_ID):
    """Require an explicit, current macOS profile; never copy its entitlements wholesale."""
    if "OSX" not in profile.get("Platform", []):
        raise ValueError("A macOS provisioning profile is required.")
    expiry = profile.get("ExpirationDate")
    if not isinstance(expiry, datetime.datetime) or expiry <= now:
        raise ValueError("The provisioning profile is expired or has no expiry.")
    entitlements = profile.get("Entitlements", {})
    team = entitlements.get(TEAM_ID, "")
    application = entitlements.get(APPLICATION_ID, "")
    prefixes = profile.get("ApplicationIdentifierPrefix", [])
    if not team or team not in profile.get("TeamIdentifier", []):
        raise ValueError("The profile's team identity is missing or inconsistent.")
    if not any(application == f"{prefix}.{bundle_id}" for prefix in prefixes):
        raise ValueError(f"Use an explicit provisioning profile for {bundle_id}, not a wildcard.")
    return application, team


def validate_certificate(profile, identity, identities):
    if not re.fullmatch(r"[0-9A-Fa-f]{40}", identity):
        raise ValueError("Select the exact 40-character identity hash from security find-identity -v -p codesigning.")
    available = re.findall(r'\b([0-9A-Fa-f]{40})\s+"', identities)
    if identity.upper() not in {value.upper() for value in available}:
        raise ValueError("The selected valid signing identity is not installed on this Mac.")
    # SHA-1 is Apple's certificate selector here, not a cryptographic trust check.
    allowed = {hashlib.sha1(cert).hexdigest().upper() for cert in profile.get("DeveloperCertificates", [])}
    if identity.upper() not in allowed:
        raise ValueError("The selected signing certificate is not authorized by this profile.")


def wallet_entitlements(base, application, team):
    if "keychain-access-groups" in base:
        raise ValueError("Minutus must use its private default Keychain access group.")
    return {**base, APPLICATION_ID: application, TEAM_ID: team}


def verification_requirement(profile, bundle_id=BUNDLE_ID):
    certificates = profile.get("DeveloperCertificates", [])
    if not certificates:
        raise ValueError("The profile authorizes no signing certificates.")
    matches = " or ".join(
        f'certificate leaf = H"{hashlib.sha1(cert).hexdigest()}"' for cert in certificates
    )
    # codesign treats an unprefixed argument as a filename, not requirement text.
    return f'=anchor apple generic and identifier "{bundle_id}" and ({matches})'


def signed_entitlements(application):
    output = capture("codesign", "--display", "--entitlements", "-", str(application))
    if output.lstrip().startswith((b"<?xml", b"bplist")):
        return plistlib.loads(output)
    return parse_entitlement_tree(output.decode("utf-8"))


def parse_entitlement_tree(output):
    """Parse macOS 26's closed, human-readable codesign entitlement form."""
    lines = output.splitlines()
    if not lines or lines.pop(0) != "[Dict]":
        raise ValueError("codesign returned an unsupported entitlement format.")
    entitlements = {}
    while lines:
        key_line = lines.pop(0)
        if not key_line.startswith("\t[Key] ") or not lines or lines.pop(0) != "\t[Value]":
            raise ValueError("codesign returned malformed entitlement metadata.")
        if not lines:
            raise ValueError("codesign omitted an entitlement value.")
        value_line = lines.pop(0)
        key = key_line.removeprefix("\t[Key] ")
        if key in entitlements:
            raise ValueError("codesign returned a duplicate entitlement.")
        if value_line.startswith("\t\t[String] "):
            value = value_line.removeprefix("\t\t[String] ")
        elif value_line == "\t\t[Bool] true":
            value = True
        elif value_line == "\t\t[Bool] false":
            value = False
        else:
            raise ValueError("codesign returned an unsupported entitlement value.")
        entitlements[key] = value
    return entitlements


def bundle_identity(application):
    info = plistlib.loads((application / "Contents/Info.plist").read_bytes())
    bundle_id = info.get("CFBundleIdentifier")
    if bundle_id not in (BUNDLE_ID, STAGING_BUNDLE_ID):
        raise ValueError("The app must have the production or staging Minutus identifier.")
    return bundle_id


def verify(application):
    bundle_id = bundle_identity(application)
    profile_path = application / "Contents/embedded.provisionprofile"
    profile = plistlib.loads(decode_profile(profile_path))
    app_id, team = profile_identity(profile, datetime.datetime.now(datetime.timezone.utc).replace(tzinfo=None), bundle_id)
    actual = signed_entitlements(application)
    expected = wallet_entitlements(
        plistlib.loads((REPOSITORY / "src-tauri/Entitlements.plist").read_bytes()), app_id, team
    )
    if actual != expected:
        raise ValueError("Signed wallet entitlements differ from the reviewed policy and profile identity.")
    requirement = verification_requirement(profile, bundle_id)
    subprocess.run(["codesign", "--verify", "--strict", "--test-requirement", requirement, str(application)], check=True)
    details = subprocess.run(["codesign", "--display", "--verbose=4", str(application)], check=True, capture_output=True).stderr.decode()
    if f"TeamIdentifier={team}" not in details.splitlines():
        raise ValueError("The signing certificate's team differs from the profile.")


def sign(application, identity, profile_path):
    bundle_id = bundle_identity(application)
    # Complete metadata checks before touching the app or invoking codesign.
    profile_bytes = profile_path.read_bytes()
    profile = plistlib.loads(decode_profile(profile_path))
    app_id, team = profile_identity(profile, datetime.datetime.now(datetime.timezone.utc).replace(tzinfo=None), bundle_id)
    validate_certificate(profile, identity, capture("security", "find-identity", "-v", "-p", "codesigning").decode())
    embedded = application / "Contents/embedded.provisionprofile"
    if embedded.exists():
        raise ValueError("Use a freshly built app; an embedded provisioning profile already exists.")
    base = plistlib.loads((REPOSITORY / "src-tauri/Entitlements.plist").read_bytes())
    with tempfile.TemporaryDirectory(prefix="zeta-wallet-signing-") as temporary:
        entitlements = Path(temporary) / "Wallet.entitlements.plist"
        entitlements.write_bytes(plistlib.dumps(wallet_entitlements(base, app_id, team)))
        embedded.write_bytes(profile_bytes)
        environment = {**os.environ, "ZETA_APP_ENTITLEMENTS": str(entitlements)}
        subprocess.run(["sh", str(REPOSITORY / "scripts/sign-macos-bundle.sh"), str(application), identity], check=True, env=environment)
    verify(application)
    subprocess.run(["sh", str(REPOSITORY / "scripts/verify-macos-bundle.sh"), str(application)], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    signing = commands.add_parser("sign", help="Sign a fresh debug/release app bundle (does not launch it)")
    signing.add_argument("application", type=Path)
    signing.add_argument("identity", help="Exact SHA-1 certificate identity from security find-identity")
    signing.add_argument("profile", type=Path)
    checking = commands.add_parser("verify", help="Verify the signed wallet identity without reading keys")
    checking.add_argument("application", type=Path)
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("Wallet app signing requires macOS.")
    application = args.application.absolute()
    if application.name not in ("Minutus.app", "Minutus Staging.app") or application.is_symlink():
        parser.error("Expected a non-symlink Minutus.app or Minutus Staging.app bundle.")
    try:
        if args.command == "sign":
            sign(application, args.identity, args.profile.resolve())
        else:
            verify(application)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        # Avoid emitting full subprocess output, profile contents or certificate data.
        detail = str(error) if isinstance(error, ValueError) else "A signing prerequisite or platform command failed; check the certificate, profile and fresh app bundle."
        print(f"Wallet signing stopped: {detail}", file=sys.stderr)
        return 1
    print("Wallet signing verified. Live Keychain creation/unlock still requires owner testing.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
