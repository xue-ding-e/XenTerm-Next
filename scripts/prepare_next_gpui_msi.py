"""Validate and harvest a complete, source-bound GPUI Windows release bundle."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import struct
import uuid
import xml.etree.ElementTree as ET
from packaging_version import validate_manifest_version, version_details

NS = 'http://schemas.microsoft.com/wix/2006/wi'
ET.register_namespace('', NS)
REGISTRY = r'Software\XenTerm-Next\GPUI\Installer'
# Permanent Next GPUI namespace: never vary by source SHA, build or temp path.
COMPONENT_NAMESPACE = uuid.UUID('7548A7CB-F70B-4AE7-A5F0-F952D45AA826')


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
    seen_paths = set()
    parents = {child: parent for parent in root.iter() for child in parent}
    # Heat -cg emits components under ComponentGroup, referring to a separate
    # directory tree by ID. Resolve that tree, never infer a target from Source.
    directories = {}
    for node in root.iter():
        if node.tag in (q('Directory'), q('DirectoryRef')):
            ident = node.get('Id')
            if not ident or ident in directories:
                raise ValueError('Missing or ambiguous harvested directory ID')
            directories[ident] = node
    for component in root.iter(q('Component')):
        files = component.findall(q('File'))
        if len(files) != 1:
            raise ValueError('Expected exactly one file per harvested component')
        file = files[0]
        source = file.get('Source', '').replace('\\', '/')
        prefix = '$(var.SourceDir)/'
        relative = source[len(prefix):] if source.startswith(prefix) else ''
        parts = relative.split('/')
        if (not relative or any(part in ('', '.', '..') for part in parts)
                or any(char in relative for char in ':$*?')):
            raise ValueError('Expected safe SourceDir-relative harvested file path')
        # This product's reviewed payload uses portable ASCII names only.
        # Reject unsupported Windows aliases rather than claim full NTFS folding.
        reserved = {'CON', 'PRN', 'AUX', 'NUL'} | {
            f'{prefix}{number}' for prefix in ('COM', 'LPT') for number in range(1, 10)}
        if any(not re.fullmatch(r'[A-Za-z0-9_.-]+', part)
               or part.endswith('.') or part.split('.')[0].upper() in reserved
               for part in parts):
            raise ValueError('Unsupported Windows harvested file name')
        installed = [file.get('Name', parts[-1])]
        ancestor = parents.get(component)
        if component.get('Directory') is not None:
            if ancestor is not group:
                raise ValueError('Ambiguous harvested component directory ownership')
            ancestor = directories.get(component.get('Directory'))
        while ancestor is not None and ancestor.tag == q('Directory'):
            installed.insert(0, ancestor.get('Name', ''))
            ancestor = parents.get(ancestor)
        if (ancestor is None or ancestor.tag != q('DirectoryRef')
                or ancestor.get('Id') != 'INSTALLFOLDER'
                or '/'.join(installed).lower() != relative.lower()):
            raise ValueError('Harvest source and installed relative path differ')
        # Windows installation paths are case-insensitive. Identity follows the
        # installed relative path, not Heat IDs or a temporary extraction root.
        identity = relative.casefold()
        if identity in seen_paths:
            raise ValueError('Duplicate Windows harvested file path')
        seen_paths.add(identity)
        component.set('Guid', str(uuid.uuid5(COMPONENT_NAMESPACE, 'file:' + identity)).upper())
        file.set('KeyPath', 'no')
        registry_name = 'File' + hashlib.sha256(identity.encode('utf-8')).hexdigest()
        # WiX cannot autogenerate a GUID for file + registry-keypath components.
        # Keep HKCU keypaths for per-user ICE validation; supply a stable GUID.
        ET.SubElement(component, q('RegistryValue'), {
            'Root': 'HKCU', 'Key': REGISTRY + r'\Files', 'Name': registry_name,
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
