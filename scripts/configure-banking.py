#!/usr/bin/env python3
"""Generate local iOS OAuth configuration; never contacts a provider."""
import argparse
import json
from pathlib import Path
import plistlib
import re
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parent.parent


def artifacts(redirect, team, bundle, output):
    url = urlsplit(redirect)
    if (url.scheme != 'https' or not url.hostname or url.username or url.password
            or url.port is not None or url.netloc.lower() != url.hostname or url.query or url.fragment
            or any(c.isspace() for c in redirect)
            or any(not re.fullmatch(r'[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?', label) for label in url.hostname.split('.'))
            or '.' not in url.hostname or '..' in url.hostname
            or not re.fullmatch(r'/[A-Za-z0-9/_-]*', url.path)
            or any(part in {'.', '..'} for part in url.path.split('/'))):
        raise ValueError('Use an HTTPS domain and an explicit plain path, without credentials, port, query, fragment, or wildcards.')
    if not re.fullmatch(r'[A-Z0-9]{10}', team):
        raise ValueError('Apple team ID must contain exactly 10 uppercase letters or digits.')
    if not re.fullmatch(r'[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+', bundle):
        raise ValueError('Bundle ID must be a dot-separated Apple bundle identifier.')
    entitlement_path = str((output / 'MeshPay.entitlements').resolve())
    if any(c in entitlement_path for c in '\n\r"$'):
        raise ValueError('Output path contains characters unsupported by Xcode configuration.')
    aasa = {'applinks': {'details': [{'appIDs': [f'{team}.{bundle}'], 'components': [{'/': url.path}]}]}}
    entitlements = plistlib.loads((ROOT / 'ios/MeshPay/MeshPay.entitlements').read_bytes())
    entitlements['com.apple.developer.associated-domains'] = [f'applinks:{url.hostname}']
    config = (f'DEVELOPMENT_TEAM = {team}\nMESH_APP_BUNDLE_ID = {bundle}\n'
              f'MESH_ENTITLEMENTS = {entitlement_path}\nCODE_SIGN_STYLE = Automatic\n')
    return {
        'apple-app-site-association': (json.dumps(aasa, indent=2) + '\n').encode(),
        'MeshPay.entitlements': plistlib.dumps(entitlements),
        'ios.xcconfig': config.encode(),
        'configuration.json': (json.dumps({'redirect_uri': redirect, 'team_id': team, 'bundle_id': bundle}, indent=2) + '\n').encode(),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--redirect-uri', required=True)
    parser.add_argument('--team-id', required=True)
    parser.add_argument('--bundle-id', default='app.mesh.sandbox')
    parser.add_argument('--output', type=Path, default=ROOT / '.local/banking')
    parser.add_argument('--check', action='store_true', help='Validate existing files without writing.')
    args = parser.parse_args()
    try:
        files = artifacts(args.redirect_uri, args.team_id, args.bundle_id, args.output)
        conflicts = [name for name, data in files.items() if (args.output / name).exists() and (args.output / name).read_bytes() != data]
        if conflicts:
            raise ValueError('Existing configuration differs; choose another --output directory. No files changed.')
        if args.check:
            if any(not (args.output / name).is_file() for name in files):
                raise ValueError('Configuration files are missing; run without --check first.')
        else:
            args.output.mkdir(parents=True, exist_ok=True)
            for name, data in files.items():
                if not (args.output / name).exists():
                    with (args.output / name).open('xb') as handle:
                        handle.write(data)
    except (ValueError, OSError) as error:
        parser.exit(1, f'Configuration failed: {error}\n')
    print('Local OAuth configuration verified.' if args.check else 'Local OAuth configuration created. Follow docs/BANKING.md to host AASA and register the redirect URI.')


if __name__ == '__main__':
    main()
