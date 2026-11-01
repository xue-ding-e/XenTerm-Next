#!/usr/bin/env python3
"""Stage a distribution without invoking macOS tools or modifying the source app."""
import argparse
import pathlib
import plistlib
import re
import shutil
import xml.etree.ElementTree as ET

HERE = pathlib.Path(__file__).resolve().parent

def prepare(app: pathlib.Path, version: str, output: pathlib.Path) -> None:
    if not re.fullmatch(r"[0-9]+(?:\.[0-9]+){0,3}", version):
        raise ValueError("Installer version must be numeric, for example 0.4.2")
    if app.name != "xenterm.app" or app.is_symlink() or not app.is_dir():
        raise ValueError("Expected a real xenterm.app directory")
    with (app / "Contents/Info.plist").open("rb") as source:
        info = plistlib.load(source)
    if info.get("CFBundleIdentifier") != "dev.xenterm.xenterm":
        raise ValueError("Unexpected bundle identifier")
    if info.get("CFBundleExecutable") != "xenterm":
        raise ValueError("Unexpected bundle executable")
    binary = app / "Contents/MacOS/xenterm"
    if not binary.is_file() or binary.is_symlink() or not binary.stat().st_mode & 0o111:
        raise ValueError("Missing executable app binary")
    output.mkdir(parents=True, exist_ok=False)
    shutil.copytree(app, output / "payload/Applications/xenterm.app", symlinks=True)
    for name in ("app-scripts", "cli-scripts", "resources"):
        shutil.copytree(HERE / name, output / name)
    with (output / "components.plist").open("wb") as stream:
        plistlib.dump([{"RootRelativeBundlePath": "Applications/xenterm.app",
                       "BundleIsRelocatable": False,
                       "BundleIsVersionChecked": True,
                       "BundleOverwriteAction": "upgrade"}], stream)
    root = ET.Element("installer-gui-script", minSpecVersion="2")
    ET.SubElement(root, "title").text = "XenTerm"
    ET.SubElement(root, "options", customize="always", **{"require-scripts": "true"})
    ET.SubElement(root, "domains", enable_anywhere="false", enable_currentUserHome="false", enable_localSystem="true")
    outline = ET.SubElement(root, "choices-outline")
    for name in ("app", "cli"):
        ET.SubElement(outline, "line", choice=name)
        attrs = dict(id=name, title=name + "Title", description=name + "Description", start_selected="true")
        if name == "app":
            attrs["enabled"] = "false"
        choice = ET.SubElement(root, "choice", **attrs)
        package_id = "dev.xenterm.installer." + name
        ET.SubElement(choice, "pkg-ref", id=package_id)
        ET.SubElement(root, "pkg-ref", id=package_id, version=version, auth="Root").text = name + ".pkg"
    ET.ElementTree(root).write(output / "distribution.xml", encoding="utf-8", xml_declaration=True)

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("app", type=pathlib.Path)
    parser.add_argument("version")
    parser.add_argument("output", type=pathlib.Path)
    args = parser.parse_args()
    prepare(args.app, args.version, args.output)
