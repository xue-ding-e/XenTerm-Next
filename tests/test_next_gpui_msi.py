"""Portable unit checks; real MSI/GUI lifecycle belongs to Windows runner."""
import json
from pathlib import Path
import struct
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
from prepare_next_gpui_msi import validate_bundle, convert_harvest, installer_version, NS

SHA = 'a' * 40


class PackagingTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for name in ('LICENSE', 'THIRD_PARTY_NOTICES.md'):
            (self.root / name).write_text('fixture license')
        (self.root / 'licenses').mkdir()
        (self.root / 'licenses' / 'NOTICE').write_text('fixture notice')
        self.manifest = dict(repository='xue-ding-e/XenTerm-Next', sha=SHA,
                             platform='windows-x86_64', version_scheme='cargo-legacy',
                             package_version='0.7.3', runtime_version='0.7.3',
                             version_timestamp=None, binary_version='xenterm 0.7.3')
        (self.root / 'BUILD.json').write_text(json.dumps(self.manifest))
        # Structural fixture only, never used as an installer or GUI acceptance.
        data = bytearray(512)
        data[:2] = b'MZ'
        struct.pack_into('<I', data, 0x3c, 128)
        data[128:132] = b'PE\0\0'
        struct.pack_into('<H', data, 132, 0x8664)
        struct.pack_into('<H', data, 152, 0x20b)
        struct.pack_into('<H', data, 220, 2)
        (self.root / 'xenterm.exe').write_bytes(data)

    def test_beta_builds_are_strictly_ordered_with_same_cargo_version(self):
        old = dict(package_version='0.7.3', version_timestamp=1793303184)
        new = dict(package_version='0.7.3', version_timestamp=1793303185)
        parsed = lambda m: tuple(map(int, installer_version(m).split('.')))
        self.assertLess(parsed(old), parsed(new))
        for epoch in (1, 65535, 65536, 16777215, 16777216, 0xffffffff):
            fields = parsed(dict(version_timestamp=epoch))
            self.assertEqual((fields[0] << 24) | (fields[1] << 16) | fields[2], epoch)
        for epoch in (None, True, 0, 0x100000000):
            with self.assertRaises(ValueError):
                installer_version(dict(version_timestamp=epoch))

    def test_bundle_shape(self):
        self.assertEqual(validate_bundle(self.root, SHA), self.manifest)

    def test_wrong_source(self):
        with self.assertRaisesRegex(ValueError, 'identity'):
            validate_bundle(self.root, 'b' * 40)

    def test_console_stub(self):
        data = bytearray((self.root / 'xenterm.exe').read_bytes())
        struct.pack_into('<H', data, 220, 3)
        (self.root / 'xenterm.exe').write_bytes(data)
        with self.assertRaisesRegex(ValueError, 'GUI PE'):
            validate_bundle(self.root, SHA)

    def test_missing_license(self):
        (self.root / 'LICENSE').unlink()
        with self.assertRaisesRegex(ValueError, 'payload file'):
            validate_bundle(self.root, SHA)

    def test_user_profile_rejected(self):
        (self.root / 'config').mkdir()
        with self.assertRaisesRegex(ValueError, 'profiles'):
            validate_bundle(self.root, SHA)

    def test_harvest_per_user(self):
        path = self.root / 'files.wxs'
        path.write_text(f'<Wix xmlns="{NS}"><Fragment><DirectoryRef Id="INSTALLFOLDER"><Component Id="FileA" Guid="*"><File Id="A" Source="$(var.SourceDir)\\xenterm.exe" KeyPath="yes"/></Component></DirectoryRef></Fragment><Fragment><ComponentGroup Id="GPUIFiles"><ComponentRef Id="FileA"/></ComponentGroup></Fragment></Wix>')
        convert_harvest(path)
        root = ET.parse(path).getroot()
        q = lambda name: f'{{{NS}}}{name}'
        self.assertEqual(root.find(f'.//{q("File")}').attrib['KeyPath'], 'no')
        for registry in root.iter(q('RegistryValue')):
            self.assertEqual(registry.attrib['Root'], 'HKCU')
            self.assertIn('GPUI', registry.attrib['Key'])
        self.assertEqual(len(list(root.iter(q('RemoveFolder')))), 1)

    def test_harvest_guid_tracks_only_windows_relative_path(self):
        def converted(source, ident='FileA'):
            path = self.root / 'identity.wxs'
            relative = source.replace('\\', '/').split('$(var.SourceDir)/')[-1]
            directories = relative.split('/')[:-1]
            begin = ''.join(f'<Directory Id="D{i}" Name="{name}">' for i, name in enumerate(directories))
            end = '</Directory>' * len(directories)
            path.write_text(f'<Wix xmlns="{NS}"><Fragment><DirectoryRef Id="INSTALLFOLDER">{begin}<Component Id="{ident}" Guid="*"><File Id="A" Source="{source}"/></Component>{end}</DirectoryRef></Fragment><Fragment><ComponentGroup Id="GPUIFiles"/></Fragment></Wix>', encoding='utf-8')
            convert_harvest(path)
            c = ET.parse(path).find(f'.//{{{NS}}}Component')
            return c.attrib['Guid'], c.find(f'{{{NS}}}RegistryValue').attrib['Name']
        first = converted('$(var.SourceDir)/licenses/NOTICE')
        self.assertNotEqual(first[0], '*')
        import uuid
        self.assertEqual(uuid.UUID(first[0]).version, 5)
        self.assertEqual(first, converted('$(var.SourceDir)/LICENSES/notice', 'ChangedHeatId'))
        self.assertEqual(first, converted('$(var.SourceDir)\\licenses\\NOTICE'))
        self.assertNotEqual(first, converted('$(var.SourceDir)/other/NOTICE'))
        for source in ('', 'C:/temp/NOTICE', '$(var.SourceDir)/../NOTICE',
                       '$(var.SourceDir)//NOTICE', '$(var.SourceDir)/a:NOTICE',
                       '$(var.SourceDir)/./NOTICE', '$(var.SourceDir)/$bad',
                       '$(var.SourceDir)/NUL.txt', '$(var.SourceDir)/NOTICE.',
                       '$(var.SourceDir)/NOTICE ', '$(var.SourceDir)/Straße'):
            with self.subTest(source=source), self.assertRaises(ValueError):
                converted(source)

    def test_harvest_rejects_ambiguous_components(self):
        for files in ('', '<File Source="$(var.SourceDir)/a"/><File Source="$(var.SourceDir)/b"/>'):
            path = self.root / 'bad.wxs'
            path.write_text(f'<Wix xmlns="{NS}"><ComponentGroup Id="GPUIFiles"/><Component Id="A" Guid="*">{files}</Component></Wix>')
            with self.assertRaisesRegex(ValueError, 'exactly one'):
                convert_harvest(path)
        path.write_text(f'<Wix xmlns="{NS}"><ComponentGroup Id="GPUIFiles"/><DirectoryRef Id="INSTALLFOLDER"><Component Id="A"><File Source="$(var.SourceDir)/X.exe"/></Component><Component Id="B"><File Source="$(var.SourceDir)/x.EXE"/></Component></DirectoryRef></Wix>')
        with self.assertRaisesRegex(ValueError, 'Duplicate Windows'):
            convert_harvest(path)

    def test_harvest_rejects_source_target_mismatch(self):
        for directory, name in (('OTHER', 'x.exe'), ('INSTALLFOLDER', 'renamed.exe')):
            path = self.root / 'mismatch.wxs'
            path.write_text(f'<Wix xmlns="{NS}"><ComponentGroup Id="GPUIFiles"/><DirectoryRef Id="{directory}"><Component Id="A"><File Name="{name}" Source="$(var.SourceDir)/x.exe"/></Component></DirectoryRef></Wix>')
            with self.assertRaisesRegex(ValueError, 'installed relative path differ'):
                convert_harvest(path)

    def test_real_heat_component_group_directory_references(self):
        # Exact Heat output from run 41490907237, artifact 13163722928.
        fixture = Path(__file__).parent / 'fixtures/gpui-heat-component-group.wxs'
        path = self.root / 'real-heat.wxs'
        path.write_bytes(fixture.read_bytes())
        convert_harvest(path)
        components = [c for c in ET.parse(path).findall(f'.//{{{NS}}}Component')
                      if c.find(f'{{{NS}}}File') is not None]
        self.assertEqual(len(components), 7)
        self.assertEqual(len({c.get('Guid') for c in components}), 7)
        self.assertTrue(all(c.get('Guid') != '*' for c in components))
        self.assertTrue(all(c.find(f'{{{NS}}}RegistryValue').get('Root') == 'HKCU' for c in components))

    def test_referenced_directory_rejects_unresolved_ambiguous_and_outside_paths(self):
        fixture = (Path(__file__).parent / 'fixtures/gpui-heat-component-group.wxs').read_text(encoding='utf-8-sig')
        cases = {
            'unresolved': fixture.replace('Directory="INSTALLFOLDER"', 'Directory="UNKNOWN"', 1),
            'outside root': fixture.replace('Id="INSTALLFOLDER"', 'Id="OTHER"'),
            'renamed directory': fixture.replace('Name="licenses"', 'Name="other"'),
            'traversal': fixture.replace('Name="licenses"', 'Name="../licenses"'),
            'external source': fixture.replace('$(var.SourceDir)\\BUILD.json', 'C:\\outside\\BUILD.json'),
            'duplicate directory': fixture.replace('<DirectoryRef Id="INSTALLFOLDER">', '<DirectoryRef Id="INSTALLFOLDER"><Directory Id="INSTALLFOLDER" Name="other" />'),
            'nested conflicting directory': fixture.replace('<ComponentGroup Id="GPUIFiles">', '<ComponentGroup Id="GPUIFiles"><Directory Id="BAD" Name="bad">').replace('</ComponentGroup>', '</Directory></ComponentGroup>'),
        }
        for label, content in cases.items():
            with self.subTest(label=label):
                path = self.root / 'rejected.wxs'
                path.write_text(content, encoding='utf-8')
                before = path.read_bytes()
                with self.assertRaises(ValueError):
                    convert_harvest(path)
                self.assertEqual(path.read_bytes(), before)

    def test_independent_identity(self):
        root = ET.parse(Path(__file__).resolve().parents[1] / 'wix/next-gpui.wxs').getroot()
        q = lambda name: f'{{{NS}}}{name}'
        product = root.find(q('Product'))
        self.assertEqual(product.attrib['UpgradeCode'], '7548A7CB-F70B-4AE7-A5F0-F952D45AA826')
        self.assertEqual(product.find(q('Package')).attrib['InstallScope'], 'perUser')
        self.assertNotIn('AllowSameVersionUpgrades', product.find(q('MajorUpgrade')).attrib)
        self.assertEqual(product.find(q('Upgrade')).find(q('UpgradeVersion')).attrib['IncludeMinimum'], 'yes')
        self.assertFalse(any('CloseApplication' in node.tag for node in root.iter()))
        self.assertFalse(list(root.iter(q('Environment'))))


if __name__ == '__main__':
    unittest.main()
