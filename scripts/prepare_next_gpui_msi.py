"""Validate and harvest a complete, source-bound GPUI Windows release bundle."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import struct
import xml.etree.ElementTree as ET
from packaging_version import validate_manifest_version, version_details

NS = 'http://schemas.microsoft.com/wix/2006/wi'
ET.register_namespace('', NS)
REGISTRY = r'Software\XenTerm-Next\GPUI\Installer'


def installer_version(manifest):
    # Windows Installer compares only 8/8/16-bit numeric fields. Encode the
    # source-bound runtime's release epoch without truncating seconds. Cargo's
    # version stays separate: many real beta sources share one Cargo version.
    epoch = manifest.get('version_timestamp')
    if type(epoch) is not int or not 0 < epoch <= 0xffffffff:
        raise ValueError('GPUI beta MSI requires a 32-bit runtime release timestamp')
    return f'{epoch >> 24}.{(epoch >> 16) & 255}.{epoch & 65535}'


def validate_bundle(bundle, source_sha):
    if not re.fullmatch('[0-9a-f]{40}', source_sha):
        raise ValueError('Expected full product source SHA')
    for name in ('xenterm.exe', 'LICENSE', 'THIRD_PARTY_NOTICES.md', 'BUILD.json'):
        path = bundle / name
        if not path.is_file() or path.is_symlink() or path.stat().st_size == 0:
            raise ValueError('Missing regular payload file: ' + name)
    if not (bundle / 'licenses').is_dir() or not any((bundle / 'licenses').rglob('*')):
        raise ValueError('Missing third-party licenses')
    if (bundle / 'config').exists():
        raise ValueError('User profiles must never be harvested into the MSI')
    for path in bundle.rglob('*'):
        if path.is_symlink():
            raise ValueError('Bundle links are forbidden')
    manifest = json.loads((bundle / 'BUILD.json').read_text(encoding='utf-8-sig'))
    if (manifest.get('repository') != 'xue-ding-e/XenTerm-Next' or
            manifest.get('sha') != source_sha or manifest.get('platform') != 'windows-x86_64'):
        raise ValueError('GPUI bundle source identity mismatch')
    expected = version_details(manifest.get('runtime_version'), manifest.get('package_version'),
                               manifest.get('version_scheme'), source_sha)
    validate_manifest_version(manifest, source_sha, expected)
    data = (bundle / 'xenterm.exe').read_bytes()
    try:
        offset = struct.unpack_from('<I', data, 0x3c)[0]
        valid = (data[:2] == b'MZ' and data[offset:offset+4] == b'PE\0\0' and
                 struct.unpack_from('<H', data, offset+4)[0] == 0x8664 and
                 struct.unpack_from('<H', data, offset+24)[0] == 0x20b and
                 struct.unpack_from('<H', data, offset+24+68)[0] == 2)
    except struct.error:
        valid = False
    if not valid:
        raise ValueError('Expected real AMD64 Windows GUI PE, not a console fixture')
    return manifest


def convert_harvest(path):
    tree = ET.parse(path)
    root = tree.getroot()
    q = lambda name: f'{{{NS}}}{name}'
    group = root.find(f'.//{q("ComponentGroup")}')
    if group is None:
        raise ValueError('Missing harvested component group')
    for component in root.iter(q('Component')):
        for file in component.findall(q('File')):
            file.set('KeyPath', 'no')
        ET.SubElement(component, q('RegistryValue'), {
            'Root': 'HKCU', 'Key': REGISTRY + r'\Files', 'Name': component.attrib['Id'],
            'Type': 'integer', 'Value': '1', 'KeyPath': 'yes'})
    # Remove only empty installer-owned directories, never user profile data.
    for directory in list(root.iter(q('Directory'))) + list(root.iter(q('DirectoryRef'))):
        ident = 'Cleanup' + hashlib.sha256(directory.attrib['Id'].encode()).hexdigest()[:24]
        component = ET.SubElement(directory, q('Component'), {'Id': ident, 'Guid': '*', 'Win64': 'yes'})
        ET.SubElement(component, q('RemoveFolder'), {'Id': ident, 'On': 'uninstall'})
        ET.SubElement(component, q('RegistryValue'), {
            'Root': 'HKCU', 'Key': REGISTRY + r'\Directories', 'Name': ident,
            'Type': 'integer', 'Value': '1', 'KeyPath': 'yes'})
        ET.SubElement(group, q('ComponentRef'), {'Id': ident})
    tree.write(path, encoding='utf-8', xml_declaration=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--bundle', type=Path)
    parser.add_argument('--source-sha')
    parser.add_argument('--harvest', type=Path)
    parser.add_argument('--installer-version', action='store_true')
    args = parser.parse_args()
    if args.harvest:
        convert_harvest(args.harvest)
    else:
        manifest = validate_bundle(args.bundle, args.source_sha)
        print(installer_version(manifest) if args.installer_version else json.dumps(manifest))
