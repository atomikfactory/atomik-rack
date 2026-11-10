"""Checks the Linux packaging files that need no Linux tools (CI runs it on every pull request).

Usage: python3 -I scripts/release/check-linux-packaging.py   (repository root)

- the Flatpak manifest parses, names the documented permissions, and every file it installs is one
  that package-flatpak.sh stages;
- the metainfo template, the file type and the metainfo rendered by linux-metainfo.sh parse as XML,
  and the id, the launchable and the desktop file agree;
- the desktop entry has the keys the packages rely on.
Needs PyYAML (preinstalled on the GitHub runners).
"""

import os
import subprocess
import sys
import tempfile
import xml.dom.minidom

import yaml

ID = "io.github.atomikfactory.AtomikRack"
DIR = "packaging/linux"


def fail(message):
    print("error: " + message, file=sys.stderr)
    sys.exit(1)


def check_manifest():
    path = f"{DIR}/{ID}.yml"
    text = open(path, encoding="utf-8").read()
    manifest = yaml.safe_load(text)
    if manifest["app-id"] != ID or manifest["command"] != "atomik":
        fail("the manifest has the wrong id or command")
    if manifest["runtime"] != "org.freedesktop.Platform":
        fail("the runtime must be org.freedesktop.Platform")
    args = set(manifest["finish-args"])
    needed = [
        "--socket=wayland", "--socket=fallback-x11", "--share=ipc", "--device=dri",
        "--socket=pulseaudio", "--device=all", "--filesystem=xdg-music",
        "--filesystem=xdg-documents", "--filesystem=home:ro", "--filesystem=~/.vst3:ro",
        "--filesystem=~/.clap:ro", "--share=network",
    ]
    for arg in needed:
        if arg not in args:
            fail(f"the manifest lacks {arg}")
    for arg in args:
        if arg.startswith("--filesystem=host") or arg == "--filesystem=home":
            fail(f"{arg} is wider than the documented permissions")
    staged = {
        "atomik-rack-linux-x86_64.tar.gz", f"{ID}.desktop", f"{ID}.mime.xml", f"{ID}.metainfo.xml",
        "icon-128.png", "icon-256.png", "icon-512.png",
    }
    module = manifest["modules"][0]
    sources = {s["path"] for s in module["sources"]}
    if sources != staged:
        fail(f"the sources differ from the files package-flatpak.sh stages: {sorted(sources ^ staged)}")
    commands = "\n".join(module["build-commands"])
    for name in staged - {"atomik-rack-linux-x86_64.tar.gz"}:
        if name not in commands:
            fail(f"the build commands never install {name}")
    if "@TARBALL_SHA256@" not in text:
        fail("the manifest lost its checksum marker")
    script = open("scripts/release/package-flatpak.sh", encoding="utf-8").read()
    for name in staged - {"atomik-rack-linux-x86_64.tar.gz"}:
        if name.startswith("icon-"):
            continue
        if name.replace(ID, "$id") not in script:
            fail(f"package-flatpak.sh never stages {name}")


def check_xml():
    for name in (f"{ID}.mime.xml", f"{ID}.metainfo.xml.in"):
        xml.dom.minidom.parse(f"{DIR}/{name}")
    with tempfile.TemporaryDirectory() as tmp:
        out = os.path.join(tmp, "m.xml")
        env = dict(os.environ, RELEASE_DATE="2026-01-02")
        subprocess.run(["sh", "scripts/release/linux-metainfo.sh", "v1.2.3-rc.1", out],
                       check=True, env=env, stdout=subprocess.DEVNULL)
        doc = xml.dom.minidom.parse(out)
        release = doc.getElementsByTagName("release")[0]
        if release.getAttribute("version") != "1.2.3~rc.1" or release.getAttribute("date") != "2026-01-02":
            fail("the releases list does not follow the label")
        if release.getAttribute("type") != "development":
            fail("a release candidate must be a development release")
        text = doc.getElementsByTagName("id")[0].firstChild.data
        if text != ID:
            fail("the metainfo id differs from the app id")
        launch = doc.getElementsByTagName("launchable")[0].firstChild.data
        if launch != f"{ID}.desktop":
            fail("the launchable is not the desktop file")
        for tag in ("content_rating", "summary", "description", "project_license"):
            if not doc.getElementsByTagName(tag):
                fail(f"the metainfo has no {tag}")


def check_desktop():
    entries = {}
    for line in open(f"{DIR}/{ID}.desktop", encoding="utf-8").read().splitlines():
        if "=" in line:
            key, value = line.split("=", 1)
            entries[key] = value
    for key, want in (("Type", "Application"), ("Icon", ID), ("Exec", "atomik %f"),
                      ("StartupWMClass", ID), ("MimeType", "application/x-atomik-project;")):
        if entries.get(key) != want:
            fail(f"the desktop entry has {key}={entries.get(key)!r}, wanted {want!r}")
    if not entries.get("Categories", "").startswith("AudioVideo;Audio;"):
        fail("the desktop entry categories must start with AudioVideo;Audio;")


check_manifest()
check_xml()
check_desktop()
print("linux packaging: ok")
