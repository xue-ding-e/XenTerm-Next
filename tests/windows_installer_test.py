#!/usr/bin/env python3
"""Portable contract checks; actual MSI execution is in windows_installer_e2e.ps1."""
import pathlib
import unittest
import xml.etree.ElementTree as ET

ROOT = pathlib.Path(__file__).resolve().parents[1]
NS = {'w': 'http://schemas.microsoft.com/wix/2006/wi'}


class WindowsInstallerContract(unittest.TestCase):
    def setUp(self):
        self.root = ET.parse(ROOT / 'wix/main.wxs').getroot()

    def test_optional_integrations_default_to_local_install(self):
        for name in ['CommandLineIntegration', 'StartMenuIntegration', 'DesktopIntegration']:
            feature = self.root.find(f'.//w:Feature[@Id="{name}"]', NS)
            self.assertIsNotNone(feature)
            self.assertEqual(feature.get('Level'), '1')
            self.assertEqual(feature.get('AllowAdvertise'), 'no')
            self.assertIsNotNone(feature.find('w:ComponentRef', NS))

    def test_path_is_additive_and_owned_by_optional_component(self):
        component = self.root.find('.//w:Component[@Id="CommandLinePath"]', NS)
        self.assertIsNotNone(component)
        entry = component.find('w:Environment', NS)
        self.assertIsNotNone(entry)
        self.assertEqual(entry.get('Name'), 'PATH')
        self.assertEqual(entry.get('Part'), 'last')
        self.assertEqual(entry.get('Permanent'), 'no')
        self.assertEqual(entry.get('Action'), 'set')
        self.assertEqual(entry.get('System'), 'yes')
        self.assertEqual(entry.get('Value'), '[APPLICATIONFOLDER]')
        self.assertEqual(component.get('Win64'), 'yes')

    def test_upgrade_preserves_choices_and_location_is_selectable(self):
        self.assertEqual(self.root.find('.//w:MajorUpgrade', NS).get('MigrateFeatures'), 'yes')
        main = self.root.find('.//w:Feature[@Id="MainProgram"]', NS)
        self.assertEqual(main.get('ConfigurableDirectory'), 'APPLICATIONFOLDER')
        self.assertEqual(main.get('Absent'), 'disallow')
        self.assertIsNotNone(self.root.find('.//w:UIRef[@Id="WixUI_FeatureTree"]', NS))
        self.assertIsNotNone(self.root.find('.//w:RegistrySearch[@Id="PreviousInstallDirRegistry"]', NS))

    def test_shortcuts_belong_only_to_optional_features(self):
        for component in ['ApplicationShortcut', 'DesktopShortcut']:
            owners = [f.get('Id') for f in self.root.findall('.//w:Feature', NS)
                      if f.find(f'w:ComponentRef[@Id="{component}"]', NS) is not None]
            self.assertEqual(len(owners), 1)
            self.assertNotEqual(owners[0], 'MainProgram')


if __name__ == '__main__':
    unittest.main()
