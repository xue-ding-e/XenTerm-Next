"""Source-level MSI isolation checks; not Windows Installer execution tests."""
from pathlib import Path
import unittest
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]
NS = {'w': 'http://schemas.microsoft.com/wix/2006/wi'}


class MsiIdentityTests(unittest.TestCase):
    def setUp(self):
        self.root = ET.parse(ROOT / 'packaging/windows/main.wxs').getroot()
        self.product = self.root.find('w:Product', NS)

    def test_distinct_upgrade_family_and_user_writable_default(self):
        self.assertEqual(self.product.get('Name'), 'XenTerm Next Flutter')
        self.assertNotEqual(self.product.get('UpgradeCode'), '6E8F2A1B-3C4D-4E5F-9A0B-1C2D3E4F5A6B')
        package = self.product.find('w:Package', NS)
        self.assertEqual(package.get('InstallScope'), 'perUser')
        self.assertEqual(package.get('InstallPrivileges'), 'limited')
        folder = self.product.find(".//w:Directory[@Id='LocalAppDataFolder']/w:Directory[@Id='INSTALLFOLDER']", NS)
        self.assertIsNotNone(folder)
        self.assertEqual(folder.get('Name'), 'XenTerm Next Flutter')
        self.assertIsNotNone(folder.get('ComponentGuidGenerationSeed'))

    def test_shortcuts_and_registry_do_not_own_other_products_resources(self):
        for shortcut in self.product.findall('.//w:Shortcut', NS):
            self.assertEqual(shortcut.get('Name'), 'XenTerm Next Flutter')
        for value in self.product.findall('.//w:RegistryValue', NS):
            self.assertEqual(value.get('Key'), r'Software\XenTerm-Next\Flutter')
        old = {'{7F1C4E7A-2D6B-4A8E-9C33-5B1D0A6F2E81}', '{B4D7E2C9-8A31-4F6E-9B52-6C0A1E3D8F47}'}
        self.assertFalse(old & {c.get('Guid') for c in self.product.findall('.//w:Component', NS)})
        # Name-only process matching can close a GPUI/upstream executable.
        self.assertFalse(self.product.findall('.//{http://schemas.microsoft.com/wix/UtilExtension}CloseApplication'))
        self.assertFalse(self.product.findall('.//w:RemoveFile', NS))

    def test_staging_excludes_both_profiles_and_logs(self):
        script = (ROOT / 'apps/xenterm/tool/package_windows_installer.ps1').read_text()
        self.assertIn('"/XD", "config", "config-flutter-next", "log"', script)
        self.assertIn('@("xenterm.exe", "xenterm-cli.exe", "xenterm_bridge.dll")', script)
        self.assertIn('$msiName = "xenterm-next-flutter-$Version-windows-x86_64.msi"', script)


if __name__ == '__main__':
    unittest.main()
