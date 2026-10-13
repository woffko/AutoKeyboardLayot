"""Build the Linux base package and install it for one user; never restart IBus or change input sources.

  receipt OUTPUT.json   build the release program and record what Cargo compiled (the input of
                        collect_dependency_notices.py --platform x86_64-unknown-linux-gnu)
  build ...             build the .deb from reviewed notices; it is inspected, never installed
  install-user          install into the home directory and point GNOME's IBus at the engine
  uninstall-user        remove what install-user wrote; the profile is kept

The package is the modular base, as the Windows installer is: the Russian and Estonian dictionaries
come in signed language packages (docs/licensing-notes.md), so the bundled-language feature is off.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile
import tomllib
import xml.etree.ElementTree as ElementTree

ROOT = Path(__file__).resolve().parents[1]
TARGET = 'x86_64-unknown-linux-gnu'
TARGET_DIR = 'target/linux-package'
PROGRAM = 'AutoKeyboardLayot'  # the Cargo binary target
COMMAND = 'autokeyboardlayot'  # the installed command and the name of every installed file
CARGO_OPTIONS = ['--release', '--locked', '--offline', '--no-default-features', '--bin', PROGRAM,
                 '--target', TARGET, '--target-dir', TARGET_DIR]
COMPONENT_NAME = 'org.freedesktop.IBus.AutoKeyboardLayot'
SYSTEM_COMPONENTS = '/usr/share/ibus/component'
GNOME_IBUS_UNIT = 'org.freedesktop.IBus.session.GNOME.service'
UNIT_DIRECTORIES = ('/etc/systemd/user', '/usr/local/lib/systemd/user', '/usr/lib/systemd/user')
MAINTAINER = 'woffko <2505149+woffko@users.noreply.github.com>'
HOMEPAGE = 'https://github.com/woffko/AutoKeyboardLayot'
# Loaded at run time by the settings window (winit and the OpenGL renderer), so dpkg-shlibdeps
# cannot see them: Wayland and EGL, and the X11 libraries of an X11 session.
RUNTIME_DEPENDS = ['ibus (>= 1.5)', 'curl', 'libegl1', 'libgl1', 'libwayland-client0', 'libwayland-egl1',
                   'libxkbcommon-x11-0', 'libx11-6', 'libx11-xcb1', 'libxcb1', 'libxcursor1', 'libxi6',
                   'libxrender1']
RECOMMENDS = ['zenity']  # the file chooser of the package import
DROPIN_MARKER = '# Written by AutoKeyboardLayot (tools/linux_package.py install-user).'


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def package_version(root=ROOT):
    return tomllib.loads((root / 'Cargo.toml').read_text(encoding='utf-8'))['package']['version']


def compiled_artifacts(stdout, root=ROOT):
    """The package IDs of every compiler artifact and the path of the program, from Cargo's JSON."""
    packages, program = set(), None
    expected = root / TARGET_DIR / TARGET / 'release' / PROGRAM
    for line in stdout.splitlines():
        item = json.loads(line)
        if item.get('reason') != 'compiler-artifact':
            continue
        packages.add(item['package_id'])
        if item['target']['kind'] == ['bin'] and item['target']['name'] == PROGRAM:
            if program is not None or Path(item['executable']) != expected:
                raise ValueError('unexpected program location or a second program')
            program = expected
    if program is None:
        raise ValueError('the build produced no program')
    return packages, program


def cargo_build(root=ROOT):
    result = subprocess.run(['cargo', 'build', *CARGO_OPTIONS, '--message-format=json'], cwd=root,
                            stdout=subprocess.PIPE, timeout=3600, check=True)
    return compiled_artifacts(result.stdout.decode('utf-8'), root)


def write_receipt(output, root=ROOT):
    if output.exists():
        raise SystemExit('Use a new receipt path.')
    packages, program = cargo_build(root)
    receipt = {'variant': 'base', 'target': TARGET, 'build': 'package',
               'artifacts': {'program': {'file': program.name, 'sha256': digest(program)}},
               'cargo_lock_sha256': digest(root / 'Cargo.lock'),
               'cargo_toml_sha256': digest(root / 'Cargo.toml'),
               'compiled_package_ids': sorted(packages)}
    with output.open('x', encoding='utf-8') as stream:
        json.dump(receipt, stream, indent=2)
    print(json.dumps({'program': receipt['artifacts']['program'], 'compiled_packages': len(packages)}))


def check_notice_inputs(notice_directory, receipt_path, reviewed_sha256, root=ROOT):
    """The reviewed notices and the receipt they were collected from, both bound to this source."""
    report = json.loads((notice_directory / 'notices-report.json').read_text(encoding='utf-8'))
    raw = receipt_path.read_bytes()
    receipt = json.loads(raw)
    notices = notice_directory / 'THIRD-PARTY-NOTICES.txt'
    if (not report['complete_text_collection'] or report['missing']
            or report['cargo_lock_sha256'] != digest(root / 'Cargo.lock')
            or report['build_receipt_sha256'] != hashlib.sha256(raw).hexdigest()
            or receipt.get('target') != TARGET or receipt.get('build') != 'package'
            or receipt['cargo_toml_sha256'] != digest(root / 'Cargo.toml')
            or len(reviewed_sha256) != 64 or any(c not in '0123456789abcdef' for c in reviewed_sha256)
            or not notices.is_file() or digest(notices) != reviewed_sha256):
        raise SystemExit('Notice inputs are incomplete, changed or stale.')
    return notices, receipt


def desktop_exec(executable):
    """`executable` as the program of a desktop entry's Exec key: quoted when the path needs it, then
    escaped as a desktop entry string (backslashes doubled) and for field codes (% doubled)."""
    text = str(executable)
    if not all(c.isalnum() or c in '/._-+' for c in text):
        quoted = text.replace('\\', '\\\\').replace('"', '\\"').replace('`', '\\`').replace('$', '\\$')
        text = '"' + quoted.replace('\\', '\\\\') + '"'
    return text.replace('%', '%%')


def desktop_entry(executable):
    return ('[Desktop Entry]\n'
            'Type=Application\n'
            'Name=AutoKeyboardLayot\n'
            'Comment=Settings of the keyboard layout correction\n'
            f'Exec={desktop_exec(executable)} --settings\n'
            f'Icon={COMMAND}\n'
            'Terminal=false\n'
            'Categories=Settings;\n'
            'Keywords=keyboard;layout;IBus;input;\n')


def copyright_text(license_text):
    lines = ['Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/',
             'Upstream-Name: AutoKeyboardLayot', f'Source: {HOMEPAGE}', '',
             'Files: *', 'Copyright: 2026 AutoKeyboardLayot contributors', 'License: MIT',
             'Comment: The third-party components compiled into the program, the bundled English',
             ' dictionary and the layout model are listed with their license texts in',
             f' /usr/share/doc/{COMMAND}/THIRD-PARTY-NOTICES.txt.', '', 'License: MIT']
    lines += [' ' + line if line.strip() else ' .' for line in license_text.strip('\n').splitlines()]
    return '\n'.join(lines) + '\n'


def control_text(version, installed_kib, depends):
    description = ('keyboard layout correction for IBus\n'
                   ' An IBus input method that types English, Russian and Estonian and replaces a\n'
                   ' word typed in the wrong keyboard layout when the word ends. The Russian and\n'
                   ' Estonian dictionaries are installed from signed language packages in the\n'
                   ' settings window.')
    return (f'Package: {COMMAND}\n'
            f'Version: {version}\n'
            'Architecture: amd64\n'
            f'Maintainer: {MAINTAINER}\n'
            f'Installed-Size: {installed_kib}\n'
            f'Depends: {", ".join(depends)}\n'
            f'Recommends: {", ".join(RECOMMENDS)}\n'
            'Section: utils\n'
            'Priority: optional\n'
            f'Homepage: {HOMEPAGE}\n'
            f'Description: {description}\n')


def shlib_depends(binary):
    """The library packages the program links against, from dpkg-shlibdeps."""
    with tempfile.TemporaryDirectory() as directory:
        (Path(directory) / 'debian').mkdir()
        (Path(directory) / 'debian/control').write_text(
            f'Source: {COMMAND}\n\nPackage: {COMMAND}\nArchitecture: amd64\n', encoding='utf-8')
        output = subprocess.run(['dpkg-shlibdeps', '-O', str(binary)], cwd=directory, check=True,
                                capture_output=True, text=True, timeout=120,
                                env={**os.environ, 'LC_ALL': 'C'}).stdout
    for line in output.splitlines():
        if line.startswith('shlibs:Depends='):
            return [item.strip() for item in line.removeprefix('shlibs:Depends=').split(',') if item.strip()]
    raise ValueError('dpkg-shlibdeps reported no dependencies')


def replace_file(target, write, mode):
    """Writes through a temporary file in the target's directory and renames it over the target, so
    a running engine keeps its old program and nobody reads a half-written file."""
    target.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(dir=target.parent, prefix='.' + target.name + '.')
    try:
        os.close(descriptor)
        write(Path(temporary))
        os.chmod(temporary, mode)
        os.replace(temporary, target)
    except BaseException:
        Path(temporary).unlink(missing_ok=True)
        raise


def install_program(binary, target, strip):
    def write(path):
        shutil.copyfile(binary, path)
        if strip:
            subprocess.run(['strip', '--remove-section=.comment', '--remove-section=.note', str(path)],
                           check=True, timeout=120)
    replace_file(target, write, 0o755)


def install_text(target, text):
    replace_file(target, lambda path: path.write_text(text, encoding='utf-8'), 0o644)


def install_files(layout, binary, executable, strip=True):
    """Installs the program at layout['binary'] and the files that point IBus and the desktop at
    `executable`, the path of the program once installed. The copy is stripped of symbols."""
    install_program(binary, layout['binary'], strip)
    component = subprocess.run([str(layout['binary']), '--print-component', shlex.quote(str(executable)) + ' --ibus'],
                               check=True, capture_output=True, text=True, timeout=30).stdout
    if ElementTree.fromstring(component).findtext('name') != COMPONENT_NAME:
        raise ValueError('the program printed an unexpected IBus component')
    install_text(layout['component'], component)
    install_text(layout['desktop'], desktop_entry(executable))
    replace_file(layout['icon'], lambda path: shutil.copyfile(ROOT / 'assets/app-256.png', path), 0o644)


def package_layout(root):
    return {'binary': root / 'usr/bin' / COMMAND,
            'component': root / 'usr/share/ibus/component' / f'{COMMAND}.xml',
            'desktop': root / 'usr/share/applications' / f'{COMMAND}.desktop',
            'icon': root / 'usr/share/icons/hicolor/256x256/apps' / f'{COMMAND}.png',
            'doc': root / 'usr/share/doc' / COMMAND}


def normalize_tree(root):
    """Directories 0755, the program 0755, every other file 0644; the files in their tree order."""
    files = []
    for directory, names, file_names in os.walk(root):
        names.sort()
        os.chmod(directory, 0o755)
        for name in sorted(file_names):
            path = Path(directory) / name
            os.chmod(path, 0o755 if path.parent.name == 'bin' else 0o644)
            files.append(path)
    return files


def build_package(output, notice_directory, receipt_path, reviewed_sha256, root=ROOT):
    output = (root / output).resolve()
    if output == root / 'target' or not output.is_relative_to(root / 'target'):
        raise SystemExit('Output must be a fresh child of this project target directory.')
    notices, receipt = check_notice_inputs(root / notice_directory, root / receipt_path, reviewed_sha256, root)
    output.mkdir()  # keeps every earlier build
    subprocess.run(['cargo', 'clippy', *CARGO_OPTIONS, '--', '-D', 'warnings'], cwd=root, timeout=3600, check=True)
    compiled, program = cargo_build(root)
    if not compiled.issubset(set(receipt['compiled_package_ids'])):
        raise SystemExit('The program uses dependencies outside the notice receipt.')
    stage = output / 'root'
    layout = package_layout(stage)
    install_files(layout, program, f'/usr/bin/{COMMAND}')
    layout['doc'].mkdir(parents=True)
    shutil.copyfile(notices, layout['doc'] / 'THIRD-PARTY-NOTICES.txt')
    (layout['doc'] / 'copyright').write_text(copyright_text((root / 'LICENSE').read_text(encoding='utf-8')),
                                             encoding='utf-8')
    files = normalize_tree(stage)
    installed_kib = sum((path.stat().st_size + 1023) // 1024 for path in files)
    version = package_version(root) + '-1'
    depends = shlib_depends(layout['binary']) + RUNTIME_DEPENDS
    control = stage / 'DEBIAN'
    control.mkdir(mode=0o755)
    (control / 'control').write_text(control_text(version, installed_kib, depends), encoding='utf-8')
    (control / 'md5sums').write_text(''.join(
        f'{hashlib.md5(path.read_bytes()).hexdigest()}  {path.relative_to(stage)}\n' for path in files),
        encoding='utf-8')
    for path in control.iterdir():
        os.chmod(path, 0o644)
    deb = output / f'{COMMAND}_{version}_amd64.deb'
    subprocess.run(['dpkg-deb', '--root-owner-group', '-Zxz', '--build', str(stage), str(deb)],
                   check=True, timeout=600)
    inspection = inspect_package(deb, stage, files, output / 'extracted')
    result = {'state': 'built_and_inspected_not_installed', 'published': False,
              'notices_complete_text_collection': True, 'redistribution_approval': False,
              'notice_sha256': digest(notices), 'source_manifest_sha256': digest(root / 'Cargo.toml'),
              'cargo_lock_sha256': digest(root / 'Cargo.lock'), 'compiled_package_ids': sorted(compiled),
              'depends': depends, 'inspection': inspection,
              'artifacts': [{'file': str(path.relative_to(root)), 'bytes': path.stat().st_size, 'sha256': digest(path)}
                            for path in [program, layout['binary'], deb]]}
    with (output / 'build.json').open('x', encoding='utf-8') as stream:
        json.dump(result, stream, indent=2)
    print('LINUX_PACKAGE_BUILT_AND_INSPECTED_NOT_INSTALLED ' + str(deb.relative_to(root)), flush=True)


def inspect_package(deb, stage, files, extracted):
    """Reads the package back: its fields, its exact file list, the extracted files against the
    staged ones, the extracted program's version, and the desktop entry."""
    fields = {name: subprocess.run(['dpkg-deb', '--field', str(deb), name], check=True, capture_output=True,
                                   text=True, timeout=60).stdout.strip()
              for name in ('Package', 'Version', 'Architecture')}
    listing = subprocess.run(['dpkg-deb', '--contents', str(deb)], check=True, capture_output=True,
                             text=True, timeout=60).stdout.splitlines()
    entries = {line.split()[-1].removeprefix('.') for line in listing}
    listed_files = {line.split()[-1].removeprefix('.') for line in listing if line.startswith('-')}
    expected = {'/' + str(path.relative_to(stage)) for path in files}
    if listed_files != expected or any(line.split()[1] != 'root/root' for line in listing):
        raise SystemExit('The package holds unexpected files or owners.')
    subprocess.run(['dpkg-deb', '-x', str(deb), str(extracted)], check=True, timeout=600)
    for path in files:
        if digest(extracted / path.relative_to(stage)) != digest(path):
            raise SystemExit('An extracted file differs from the staged one.')
    version = subprocess.run([str(extracted / 'usr/bin' / COMMAND), '--version'], check=True,
                             capture_output=True, text=True, timeout=30).stdout.strip()
    if not version.startswith(f'AutoKeyboardLayot {package_version()} ('):
        raise SystemExit('The extracted program reports an unexpected version.')
    desktop = shutil.which('desktop-file-validate')
    if desktop:
        subprocess.run([desktop, str(stage / f'usr/share/applications/{COMMAND}.desktop')], check=True, timeout=60)
    return {'fields': fields, 'entries': len(entries), 'files': len(listed_files),
            'extracted_program_version': version,
            'desktop_entry_validated': bool(desktop)}


def absolute_or(value, fallback):
    return Path(value) if value and Path(value).is_absolute() else fallback


def user_layout(environment):
    home = Path(environment['HOME'])
    data = absolute_or(environment.get('XDG_DATA_HOME'), home / '.local/share')
    config = absolute_or(environment.get('XDG_CONFIG_HOME'), home / '.config')
    state = absolute_or(environment.get('XDG_STATE_HOME'), home / '.local/state')
    return {'binary': home / '.local/bin' / COMMAND,
            'component': data / 'ibus/component' / f'{COMMAND}.xml',
            'desktop': data / 'applications' / f'{COMMAND}.desktop',
            'icon': data / 'icons/hicolor/256x256/apps' / f'{COMMAND}.png',
            'dropin': config / 'systemd/user' / f'{GNOME_IBUS_UNIT}.d' / f'{COMMAND}.conf',
            'profile': [config / COMMAND, data / COMMAND, state / COMMAND]}


def component_path(layout):
    return f'{SYSTEM_COMPONENTS}:{layout["component"].parent}'


def dropin_text(layout):
    value = f'IBUS_COMPONENT_PATH={component_path(layout)}'
    value = value.replace('\\', '\\\\').replace('"', '\\"').replace('%', '%%')
    return (f'{DROPIN_MARKER}\n'
            '# IBus has no per-user component directory; this adds one for GNOME\'s IBus service.\n'
            '# tools/linux_package.py uninstall-user removes it.\n'
            '[Service]\n'
            f'Environment="{value}"\n')


def refresh_ibus(run, environment, path):
    """Reloads the user's systemd units and rewrites the user's IBus engine cache for the component
    path the next IBus start uses; the running IBus is left alone (an IBus that finds a cache made
    without the user's component directory would not look there)."""
    notes = []
    environment = dict(environment)
    if path:
        environment['IBUS_COMPONENT_PATH'] = path
    else:
        environment.pop('IBUS_COMPONENT_PATH', None)
    for command in (['systemctl', '--user', 'daemon-reload'], ['ibus', 'write-cache']):
        try:
            if run(command, env=environment, timeout=120).returncode != 0:
                notes.append(f'{" ".join(command)} failed')
        except OSError:
            notes.append(f'{command[0]} is not available')
    return notes


def install_user(binary, environment, run=subprocess.run, unit_directories=UNIT_DIRECTORIES, strip=True):
    if not any((Path(directory) / GNOME_IBUS_UNIT).is_file() for directory in unit_directories):
        raise SystemExit(f'{GNOME_IBUS_UNIT} is not installed: this installation needs the IBus of a GNOME '
                         'session. On other desktops, install the .deb instead.')
    layout = user_layout(environment)
    refuse_foreign_dropin(layout)
    install_files(layout, binary, layout['binary'], strip)
    install_text(layout['dropin'], dropin_text(layout))
    notes = refresh_ibus(run, environment, component_path(layout))
    for key in ('binary', 'component', 'desktop', 'icon', 'dropin'):
        print(f'installed {layout[key]}')
    for note in notes:
        print(f'warning: {note}')
    print(f'''
Next steps (nothing below has been done):
  1. Restart IBus so it reads the engine:
       systemctl --user restart {GNOME_IBUS_UNIT}
     (or log out and back in).
  2. Add the input source "AutoKeyboardLayot": Settings > Keyboard > Input Sources > Add >
     Other. It types English, Russian and Estonian itself, so the other input sources and the
     Alt+Shift switch option (grp:alt_shift_toggle) can be removed; see docs/linux.md.
  3. Download Russian and Estonian in the settings window ({layout["binary"]} --settings, or
     "Settings" in the input source menu).''')


def refuse_foreign_dropin(layout):
    if layout['dropin'].is_file() and DROPIN_MARKER not in layout['dropin'].read_text(encoding='utf-8'):
        raise SystemExit(f'{layout["dropin"]} was not written by install-user; it is left alone.')


def uninstall_user(environment, run=subprocess.run):
    layout = user_layout(environment)
    refuse_foreign_dropin(layout)
    for key in ('dropin', 'component', 'desktop', 'icon', 'binary'):
        if layout[key].exists():
            layout[key].unlink()
            print(f'removed {layout[key]}')
    for directory in (layout['dropin'].parent, layout['component'].parent, layout['component'].parent.parent):
        try:
            directory.rmdir()
        except OSError:
            pass  # missing or not empty
    for note in refresh_ibus(run, environment, None):
        print(f'warning: {note}')
    profile = ' '.join(str(path) for path in layout['profile'])
    print(f'''
Remove the input source "AutoKeyboardLayot" in Settings > Keyboard if it is still listed, and
restart IBus (systemctl --user restart {GNOME_IBUS_UNIT}) or log out and back in.
The profile (settings, language packages, diagnostics) is kept; remove it with:
  rm -r {profile}''')


def main(arguments=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest='command', required=True)
    receipt = commands.add_parser('receipt', help='build the program and record the compiled packages')
    receipt.add_argument('output', type=Path, help='new receipt file')
    build = commands.add_parser('build', help='build and inspect the .deb from reviewed notices')
    build.add_argument('--output', required=True, help='new directory under target/')
    build.add_argument('--notice-directory', required=True)
    build.add_argument('--build-receipt', required=True)
    build.add_argument('--notice-sha256', required=True, help='reviewed SHA-256 of THIRD-PARTY-NOTICES.txt')
    install = commands.add_parser('install-user', help='install into the home directory')
    install.add_argument('--binary', type=Path, help='an already built program (default: build it)')
    commands.add_parser('uninstall-user', help='remove what install-user wrote')
    args = parser.parse_args(arguments)
    if args.command == 'receipt':
        write_receipt(args.output)
    elif args.command == 'build':
        build_package(args.output, args.notice_directory, args.build_receipt, args.notice_sha256)
    elif args.command == 'install-user':
        binary = args.binary or cargo_build()[1]
        install_user(binary, dict(os.environ))
    else:
        uninstall_user(dict(os.environ))
    return 0


if __name__ == '__main__':
    sys.exit(main())
