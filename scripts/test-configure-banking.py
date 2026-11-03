#!/usr/bin/env python3
"""Local CLI contract checks; no provider credentials or network required."""
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent


class BankingConfigurationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.output = Path(self.temp.name) / 'banking'

    def configure(self, *extra):
        return subprocess.run([sys.executable, str(ROOT / 'scripts/configure-banking.py'),
                               '--redirect-uri', 'https://bank.example.com/plaid/return/',
                               '--team-id', 'AB12CD34EF', '--output', str(self.output), *extra], capture_output=True)

    def test_exact_redirect_and_app_identity_match_entitlements(self):
        self.assertEqual(self.configure('--bundle-id', 'com.example.mesh').returncode, 0)
        aasa = json.loads((self.output / 'apple-app-site-association').read_text())
        self.assertEqual(aasa['applinks']['details'], [{'appIDs': ['AB12CD34EF.com.example.mesh'], 'components': [{'/': '/plaid/return/'}]}])
        entitlement = plistlib.loads((self.output / 'MeshPay.entitlements').read_bytes())
        self.assertEqual(entitlement['com.apple.developer.associated-domains'], ['applinks:bank.example.com'])
        self.assertIn('MESH_APP_BUNDLE_ID = com.example.mesh', (self.output / 'ios.xcconfig').read_text())
        self.assertEqual(self.configure('--bundle-id', 'com.example.mesh', '--check').returncode, 0)

    def test_invalid_input_never_creates_configuration(self):
        for value in ['http://bank.example.com/plaid/', 'https://user:pass@bank.example.com/plaid/',
                      'https://bank.example.com/plaid/?state=a', 'https://bank.example.com/plaid/#fragment',
                      'https://bank.example.com/plaid/*', 'https://bank.example.com',
                      'https://bank.example.com:443/plaid/', 'https://bank.example.com/a\nb',
                      'https://bank.example.com/%2f', 'https://bank.example.com:bad/plaid/', 'https://-bad.example.com/plaid/',
                      'https://bank.example.com:/plaid/']:
            with self.subTest(value=value):
                self.assertNotEqual(self.configure('--redirect-uri', value).returncode, 0)
                self.assertFalse(self.output.exists())
        self.assertNotEqual(self.configure('--team-id', 'bad').returncode, 0)
        self.assertNotEqual(self.configure('--bundle-id', 'bad/id').returncode, 0)
        self.assertFalse(self.output.exists())

    def test_check_and_changed_configuration_preserve_files(self):
        self.assertNotEqual(self.configure('--check').returncode, 0)
        self.assertFalse(self.output.exists())
        self.assertEqual(self.configure().returncode, 0)
        before = {p.name: p.read_bytes() for p in self.output.iterdir()}
        self.assertNotEqual(self.configure('--team-id', 'ZZ12CD34EF').returncode, 0)
        self.assertEqual(before, {p.name: p.read_bytes() for p in self.output.iterdir()})
        self.assertEqual(self.configure().returncode, 0)

    def test_device_build_uses_local_signing_and_simulator_keeps_adhoc(self):
        root = Path(self.temp.name) / 'checkout'
        scripts = root / 'scripts'
        scripts.mkdir(parents=True)
        shutil.copy(ROOT / 'scripts/ios.sh', scripts / 'ios.sh')
        for name in ['fetch-plaid.sh', 'build-rust-ios.sh']:
            (scripts / name).write_text('#!/bin/sh\nexit 0\n')
            (scripts / name).chmod(0o755)
        (scripts / 'generate-xcode.py').write_text('')
        binary = root / 'xcodebuild'
        binary.write_text('#!/bin/sh\nprintf "%s\\n" "$@"\n')
        binary.chmod(0o755)
        env = dict(os.environ, PATH=str(root) + ':' + os.environ['PATH'])
        env.pop('MESH_IOS_CONFIG', None)
        env['MESH_IOS_DESTINATION'] = 'generic/platform=iOS Simulator'
        run = lambda: subprocess.run(['bash', str(scripts / 'ios.sh')], env=env, capture_output=True, text=True)
        simulator = run()
        self.assertEqual(simulator.returncode, 0)
        self.assertIn('CODE_SIGN_IDENTITY=-', simulator.stdout)
        env['MESH_IOS_DESTINATION'] = 'generic/platform=iOS'
        self.assertNotEqual(run().returncode, 0)
        self.assertEqual(self.configure().returncode, 0)
        env['MESH_IOS_CONFIG'] = str(self.output / 'ios.xcconfig')
        device = run()
        self.assertEqual(device.returncode, 0)
        self.assertIn('-xcconfig\n' + env['MESH_IOS_CONFIG'], device.stdout)
        self.assertIn('CODE_SIGNING_REQUIRED=YES', device.stdout)
        self.assertNotIn('CODE_SIGN_IDENTITY=-', device.stdout)


if __name__ == '__main__':
    unittest.main()
