"""Developer ID release verification and notarization; no private keys in config."""

import argparse
import datetime
import importlib.util
import json
from pathlib import Path
import re
import subprocess
import tempfile

from inspect_macos_release import ROOT, MACH_MAGICS, safe_files, digest

SPEC = importlib.util.spec_from_file_location('wallet_signing', ROOT / 'scripts/macos-wallet-signing.py')
wallet = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(wallet)
APPLICATION = ROOT / 'target/aarch64-apple-darwin/release/bundle/macos/Minutus.app'


def load_configuration(path):
    try:
        raw = json.loads(path.read_text())
    except FileNotFoundError as error:
        raise ValueError('Create .minutus-release.json from .minutus-release.example.json after configuring Developer ID signing.') from error
    if not isinstance(raw, dict) or set(raw) != {'identity', 'profile', 'notary_profile'}:
        raise ValueError('Release configuration must contain only identity, profile and notary_profile.')
    if not isinstance(raw['identity'], str) or not re.fullmatch('[0-9A-Fa-f]{40}', raw['identity']):
        raise ValueError('Use the exact Developer ID Application certificate SHA-1.')
    if not isinstance(raw['profile'], str) or not Path(raw['profile']).is_absolute():
        raise ValueError('Use an absolute macOS provisioning-profile path.')
    if not isinstance(raw['notary_profile'], str) or not re.fullmatch('[A-Za-z0-9_.-]{1,100}', raw['notary_profile']):
        raise ValueError('Use the notarytool Keychain-profile name, not a password.')
    return {**raw, 'identity': raw['identity'].upper(), 'profile': Path(raw['profile'])}


def require_developer_id(identity, identities):
    entries = re.findall(r'\b([0-9A-Fa-f]{40})\s+"([^"]+)"', identities)
    if not any(checksum.upper() == identity.upper() and label.startswith('Developer ID Application:')
               for checksum, label in entries):
        raise ValueError('A valid Developer ID Application certificate/private-key pair is required; Apple Development and Apple Distribution cannot substitute.')


def preflight(config):
    identities = wallet.capture('security', 'find-identity', '-v', '-p', 'codesigning').decode()
    require_developer_id(config['identity'], identities)
    profile = wallet.plistlib.loads(wallet.decode_profile(config['profile']))
    wallet.profile_identity(profile, datetime.datetime.now(datetime.timezone.utc).replace(tzinfo=None))
    wallet.validate_certificate(profile, config['identity'], identities)
    return profile


def require_runtime_details(details, team):
    flags = re.search(r'flags=0x([0-9a-fA-F]+)', details)
    if not flags or not int(flags.group(1), 16) & 0x10000:
        raise ValueError('Nested release code lacks the hardened runtime.')
    if not re.search(r'^Authority=Developer ID Application:', details, re.MULTILINE):
        raise ValueError('Nested release code lacks a Developer ID Application signature.')
    if f'TeamIdentifier={team}' not in details.splitlines():
        raise ValueError('Nested release code uses a different developer team.')
    timestamp = re.search(r'^Timestamp=(.+)', details, re.MULTILINE)
    if not timestamp or timestamp.group(1).strip().lower() in {'none', 'not set'}:
        raise ValueError('Nested release code lacks a secure signing timestamp.')


def verify(application, config):
    profile = preflight(config)
    _, team = wallet.profile_identity(profile, datetime.datetime.now(datetime.timezone.utc).replace(tzinfo=None))
    wallet.verify(application)
    requirement = ('=anchor apple generic and certificate leaf = H"' + config['identity']
                   + '" and certificate leaf[field.1.2.840.113635.100.6.1.13] exists')
    targets = [path for path, magic in safe_files(application / 'Contents')
               if magic in MACH_MAGICS and not path.is_symlink()]
    for target in [*targets, application]:
        subprocess.run(['codesign', '--verify', '--strict', '--all-architectures', '--test-requirement', requirement, str(target)], check=True)
        result = subprocess.run(['codesign', '--display', '--verbose=4', str(target)],
                                check=True, capture_output=True, text=True)
        require_runtime_details(result.stderr, team)
        raw_entitlements = wallet.capture('codesign', '--display', '--entitlements', '-', str(target))
        entitlements = wallet.signed_entitlements(target) if raw_entitlements.strip() else {}
        if any(entitlements.get(key) for key in (
                'com.apple.security.get-task-allow', 'com.apple.security.cs.disable-library-validation',
                'com.apple.security.cs.allow-unsigned-executable-memory', 'com.apple.security.cs.allow-jit')):
            raise ValueError('Release code contains an unreviewed runtime exception.')
    return len(targets)


def submit(artifact, config, report_path):
    result = subprocess.run(['xcrun', 'notarytool', 'submit', str(artifact),
                             '--keychain-profile', config['notary_profile'], '--wait',
                             '--output-format', 'json'], check=True, capture_output=True, text=True)
    report = json.loads(result.stdout)
    report_path.write_text(json.dumps(report, indent=2) + '\n')
    if report.get('status') != 'Accepted':
        raise ValueError('Apple did not accept the notarization submission; inspect its saved ID with notarytool log.')
    subprocess.run(['xcrun', 'stapler', 'staple', str(artifact)], check=True)
    subprocess.run(['xcrun', 'stapler', 'validate', str(artifact)], check=True)


def notarize(application, config):
    verify(application, config)
    output = application.parent.parent / 'distribution'
    output.mkdir(exist_ok=True)
    version = json.loads((ROOT / 'package.json').read_text())['version']
    dmg = output / f'Minutus-{version}-arm64.dmg'
    if dmg.exists():
        raise ValueError('The versioned DMG already exists; preserve it or increment the release version.')
    with tempfile.TemporaryDirectory(prefix='minutus-notary-') as temporary:
        archive = Path(temporary) / 'Minutus.zip'
        subprocess.run(['ditto', '-c', '-k', '--keepParent', str(application), str(archive)], check=True)
        # App first: the update DMG and installer both carry its stapled ticket.
        result = subprocess.run(['xcrun', 'notarytool', 'submit', str(archive),
                                 '--keychain-profile', config['notary_profile'], '--wait',
                                 '--output-format', 'json'], check=True, capture_output=True, text=True)
        report = json.loads(result.stdout)
        (output / 'app-notarization.json').write_text(json.dumps(report, indent=2) + '\n')
        if report.get('status') != 'Accepted':
            raise ValueError('Apple did not accept the application; inspect the saved submission ID.')
        subprocess.run(['xcrun', 'stapler', 'staple', str(application)], check=True)
        subprocess.run(['xcrun', 'stapler', 'validate', str(application)], check=True)
        stage = Path(temporary) / 'disk-image'
        stage.mkdir()
        subprocess.run(['ditto', str(application), str(stage / 'Minutus.app')], check=True)
        (stage / 'Applications').symlink_to('/Applications')
        subprocess.run(['hdiutil', 'create', '-volname', 'Minutus', '-srcfolder', str(stage),
                        '-format', 'UDZO', str(dmg)], check=True)
    subprocess.run(['codesign', '--sign', config['identity'], '--timestamp', str(dmg)], check=True)
    subprocess.run(['codesign', '--verify', '--strict', str(dmg)], check=True)
    submit(dmg, config, output / 'dmg-notarization.json')
    verify(application, config)
    subprocess.run(['spctl', '--assess', '--type', 'execute', '--verbose=2', str(application)], check=True)
    subprocess.run(['spctl', '--assess', '--type', 'open', '--context', 'context:primary-signature',
                    '--verbose=2', str(dmg)], check=True)
    (output / 'notarized-release.json').write_text(json.dumps(dict(
        application=str(application), dmg=dmg.name, dmg_sha256=digest(dmg),
        local_gatekeeper_passed=True, clean_mac_gatekeeper_tested=False,
        offline_update_signature_present=False), indent=2) + '\n')
    print(f'Notarized and locally assessed: {dmg}. Clean-Mac testing remains required.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=('check', 'verify', 'notarize'))
    parser.add_argument('--config', type=Path, default=ROOT / '.minutus-release.json')
    args = parser.parse_args()
    try:
        config = load_configuration(args.config)
        if args.action == 'check':
            preflight(config)
        elif args.action == 'verify':
            verify(APPLICATION, config)
        else:
            notarize(APPLICATION, config)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(str(error) if isinstance(error, ValueError) else 'Release platform command failed; inspect the local prerequisite or submission.', flush=True)
        return 1
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
