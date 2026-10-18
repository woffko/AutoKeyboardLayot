import tempfile
import unittest
import hashlib
import json
from pathlib import Path
from collect_dependency_notices import (
    license_paths, upstream_texts, packages_from_receipt, curated_texts, bundled_data_sections,
    bundled_data_hashes, bundled_data_notices_current)


class LicensePathsTests(unittest.TestCase):
    def test_bundled_data_notices_cover_the_dictionary_and_the_layout_model(self):
        root = Path(__file__).resolve().parents[1]
        sections = bundled_data_sections(root)
        self.assertEqual([section.split('\n')[1] for section in sections],
                         ['=== Bundled English dictionary ===', '=== Bundled layout model ==='])
        notice = (root / 'data/layout-model/NOTICE.md').read_text(encoding='utf-8')
        self.assertTrue(sections[1].endswith(notice))
        self.assertIn('Creative Commons Attribution-ShareAlike 4.0', sections[1])
        self.assertIn('wordfreq', sections[1])
        self.assertIn('FrequencyWords', sections[1])

    def test_a_missing_bundled_notice_is_an_error_not_a_silent_omission(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(OSError):
                bundled_data_sections(Path(directory))
            root = Path(directory)
            (root / 'data/language-packs/en-US').mkdir(parents=True)
            (root / 'data/language-packs/en-US/LICENSE.words.md').write_text('dictionary', encoding='utf-8')
            with self.assertRaises(OSError):
                bundled_data_sections(root)

    def test_the_report_binds_each_bundled_data_notice_by_hash(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'data/language-packs/en-US').mkdir(parents=True)
            (root / 'data/layout-model').mkdir(parents=True)
            (root / 'data/language-packs/en-US/LICENSE.words.md').write_bytes(b'dictionary')
            (root / 'data/layout-model/NOTICE.md').write_bytes(b'model')
            recorded = bundled_data_hashes(root)
            self.assertEqual(recorded, [
                {'title': 'Bundled English dictionary', 'file': 'data/language-packs/en-US/LICENSE.words.md',
                 'sha256': hashlib.sha256(b'dictionary').hexdigest()},
                {'title': 'Bundled layout model', 'file': 'data/layout-model/NOTICE.md',
                 'sha256': hashlib.sha256(b'model').hexdigest()},
            ])
            report = {'bundled_data_notices': recorded}
            self.assertTrue(bundled_data_notices_current(root, report))
            # A report made before the hashes were recorded is stale, as is one missing a notice.
            self.assertFalse(bundled_data_notices_current(root, {}))
            self.assertFalse(bundled_data_notices_current(root, {'bundled_data_notices': recorded[:1]}))
            (root / 'data/layout-model/NOTICE.md').write_bytes(b'model, retrained')
            self.assertFalse(bundled_data_notices_current(root, report))

    def test_the_notice_report_records_the_bundled_data_hashes(self):
        source = (Path(__file__).resolve().parent / 'collect_dependency_notices.py').read_text(encoding='utf-8')
        self.assertEqual(source.count("'bundled_data_notices': bundled_data_hashes(root),"), 1)

    def test_changed_bundled_data_notices_make_the_installer_build_refuse_the_notices(self):
        # The installer build cannot run here; its text is checked like in test_authenticode_hook.
        source = (Path(__file__).resolve().parent / 'build_experimental_installer.py').read_text(encoding='utf-8')
        check = source.index('or not collect_dependency_notices.bundled_data_notices_current(root, report)')
        refusal = source.index("raise SystemExit('Notice inputs are incomplete, changed or stale.')")
        self.assertLess(check, refusal)
        self.assertLess(refusal, source.index('output.mkdir()'))

    def test_curated_license_only_commit_requires_exact_source_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            vcs = {'git': {'sha1': 'ca1a2f84aa386d758e98f8a609d990263932fb85'}}
            path = root / '.cargo_vcs_info.json'
            path.write_text(json.dumps(vcs), encoding='utf-8')
            package = {'name': 'simd_helpers', 'version': '0.1.0', 'manifest_path': str(root / 'Cargo.toml'),
                       'repository': 'https://github.com/lu-zero/simd_helpers', 'license': 'MIT'}
            texts = curated_texts(package)
            self.assertEqual(len(texts), 1)
            self.assertIn('Copyright (c) 2019 Luca Barbato', texts[0][1])
            for key, changed in [('repository', 'https://github.com/other/project'), ('license', 'Apache-2.0')]:
                with self.assertRaises(ValueError):
                    curated_texts({**package, key: changed})
            vcs['git']['dirty'] = True
            path.write_text(json.dumps(vcs), encoding='utf-8')
            with self.assertRaises(ValueError):
                curated_texts(package)
            vcs['git'] = {'sha1': 'f' * 40}
            path.write_text(json.dumps(vcs), encoding='utf-8')
            with self.assertRaises(ValueError):
                curated_texts(package)
            self.assertEqual(curated_texts({**package, 'version': '0.2.0'}), [])

    def test_build_receipt_scope_is_exact_and_configuration_bound(self):
        metadata = {'packages': [{'id': 'root'}, {'id': 'used'}, {'id': 'unused'}],
                    'resolve': {'root': 'root'}}
        receipt = {'variant': 'base', 'cargo_lock_sha256': 'lock', 'cargo_toml_sha256': 'manifest',
                   'compiled_package_ids': ['root', 'used']}
        self.assertEqual(packages_from_receipt(metadata, receipt, 'lock', 'manifest'),
                         [{'id': 'root'}, {'id': 'used'}])
        with self.assertRaises(ValueError):
            packages_from_receipt(metadata, receipt, 'different', 'manifest')
        receipt['compiled_package_ids'] = ['root', 'unknown']
        with self.assertRaises(ValueError):
            packages_from_receipt(metadata, receipt, 'lock', 'manifest')
        receipt['compiled_package_ids'] = ['used']
        with self.assertRaises(ValueError):
            packages_from_receipt(metadata, receipt, 'lock', 'manifest')

    def test_upstream_binding_hash_and_dirty_status(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            crate = root / 'crate'
            crate.mkdir()
            upstream = root / 'upstream'
            upstream.mkdir()
            vcs = {'git': {'sha1': 'a' * 40, 'dirty': True}}
            (crate / '.cargo_vcs_info.json').write_text(json.dumps(vcs), encoding='utf-8')
            (upstream / 'LICENSE').write_bytes(b'license text')
            package = {'name': 'example', 'version': '1.0', 'manifest_path': str(crate / 'Cargo.toml'),
                       'repository': 'https://github.com/example/project'}
            entry = {'revision': 'a' * 40, 'repository': 'example/project', 'dirty_source': True,
                     'files': [{'path': 'LICENSE', 'sha256': hashlib.sha256(b'license text').hexdigest(),
                                'source_url': 'https://raw.githubusercontent.com/example/project/' + 'a' * 40 + '/LICENSE'}]}
            texts, dirty = upstream_texts(package, entry, upstream)
            self.assertTrue(dirty)
            self.assertEqual(texts[0][1], 'license text')
            entry['revision'] = 'b' * 40
            with self.assertRaises(ValueError):
                upstream_texts(package, entry, upstream)
            entry['revision'] = 'a' * 40
            entry['files'][0]['source_url'] += '?unexpected=query'
            with self.assertRaises(ValueError):
                upstream_texts(package, entry, upstream)
            entry['files'][0]['source_url'] = entry['files'][0]['source_url'].split('?')[0]
            (upstream / 'LICENSE').write_bytes(b'changed')
            with self.assertRaises(ValueError):
                upstream_texts(package, entry, upstream)

    def test_lowercase_and_nested_license_texts(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'license-mit').write_text('mit', encoding='utf-8')
            (root / 'LICENSES').mkdir()
            (root / 'LICENSES/custom.md').write_text('custom', encoding='utf-8')
            texts = license_paths({'manifest_path': str(root / 'Cargo.toml')})
            self.assertEqual({entry[1] for entry in texts}, {'mit', 'custom'})

    def test_explicit_license_file(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'terms.txt').write_text('terms', encoding='utf-8')
            texts = license_paths({'manifest_path': str(root / 'Cargo.toml'), 'license_file': 'terms.txt'})
            self.assertEqual(texts[0][0], 'terms.txt')

    def test_external_and_binary_files_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'package').mkdir()
            (root / 'outside').write_text('outside', encoding='utf-8')
            package = {'manifest_path': str(root / 'package/Cargo.toml'), 'license_file': '../outside'}
            with self.assertRaises(ValueError):
                license_paths(package)
            (root / 'package/LICENSE').write_bytes(b'bad\0text')
            with self.assertRaises(ValueError):
                license_paths({'manifest_path': str(root / 'package/Cargo.toml')})


if __name__ == '__main__':
    unittest.main()
