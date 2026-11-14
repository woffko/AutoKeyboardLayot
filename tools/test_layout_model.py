"""Tests of the layout-model tools that need neither wordfreq, numpy nor torch.

The tools live in tools/layout_model; their training steps need the packages of
docs/layout-model.md, but what decides the inputs (layouts, sources, versions,
the notice) runs on plain Python and is tested here.
"""
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS / 'layout_model'))

import common  # noqa: E402
import notice  # noqa: E402
import prepare_vocab  # noqa: E402

CONFIG = common.load_config()


def layout(language_id, klid=None, hkl=None):
    entry = {'language_id': language_id, 'keys': {}}
    if klid:
        entry['klid'] = klid
    if hkl:
        entry['hkl'] = hkl
    return entry


def select(layouts, config=CONFIG):
    with tempfile.TemporaryDirectory() as scratch:
        path = Path(scratch) / 'layouts.json'
        path.write_text(json.dumps({'format': 1, 'layouts': layouts}), encoding='utf-8')
        return common.select_layouts(config, path)


def config_receipt(config=CONFIG):
    """The receipt prepare_vocab.py writes, from what languages.json records."""
    digests = {entry['id']: entry['sha256'] for entry in config['languages'] if 'sha256' in entry}
    return prepare_vocab.provenance(config, digests, config['wordfreq'],
                                    {entry['id']: entry['layout'] for entry in config['languages']}, {})


class Layouts(unittest.TestCase):
    def test_each_language_takes_the_layout_its_klid_names(self):
        selected = select([layout('0409', '00020409'), layout('0409', '00000409'), layout('0419', '00000419'),
                           layout('0425', '00000425')])
        self.assertEqual({key: value['klid'] for key, value in selected.items()},
                         {'en-US': '00000409', 'ru-RU': '00000419', 'et-EE': '00000425'})

    def test_an_export_with_hkls_only_gives_the_klids_they_name(self):
        selected = select([layout('0409', hkl='0x4090409'), layout('0419', hkl='0x4190419'),
                           layout('0425', hkl='0x4250425')])
        self.assertEqual(selected['ru-RU']['hkl'], '0x4190419')
        self.assertIsNone(common.exported_klid(layout('0409', hkl='0xf0010409')))

    def test_an_export_without_klids_falls_back_to_one_layout_per_language(self):
        selected = select([layout('0409'), layout('0419'), layout('0425')])
        self.assertEqual(sorted(selected), ['en-US', 'et-EE', 'ru-RU'])

    def test_a_layout_that_is_missing_or_ambiguous_stops_the_run(self):
        cases = {
            'two layouts of one language': [layout('0409'), layout('0409'), layout('0419'), layout('0425')],
            'one missing': [layout('0409'), layout('0419')],
            'a KLID twice': [layout('0409', '00000409'), layout('0419', '00000419'), layout('0425', '00000425'),
                             layout('0809', '00000809'), layout('0c09', '00000809')],
            'the named KLID absent': [layout('0409', '00020409'), layout('0419', '00000419'),
                                      layout('0425', '00000425')],
        }
        for case, layouts in cases.items():
            with self.subTest(case=case), self.assertRaises(SystemExit):
                select(layouts)


class Sources(unittest.TestCase):
    def test_a_file_source_must_have_the_recorded_sha256(self):
        with tempfile.TemporaryDirectory() as scratch:
            directory = Path(scratch)
            (directory / 'et_full.txt').write_bytes(b'tere 5\n')
            digest = hashlib.sha256(b'tere 5\n').hexdigest()
            config = json.loads(json.dumps(CONFIG))
            estonian = next(entry for entry in config['languages'] if entry['id'] == 'et-EE')
            estonian['sha256'] = digest
            self.assertEqual(prepare_vocab.source_digests(config, directory), {'et-EE': digest})
            for value in ('0' * 64, None):
                if value is None:
                    del estonian['sha256']
                else:
                    estonian['sha256'] = value
                with self.subTest(recorded=value), self.assertRaises(SystemExit):
                    prepare_vocab.source_digests(config, directory)

    def test_the_wordfreq_version_must_be_the_recorded_one(self):
        prepare_vocab.check_wordfreq({'wordfreq': '3.1.1'}, '3.1.1')
        for config, installed in (({'wordfreq': '3.1.1'}, '3.0.2'), ({}, '3.1.1')):
            with self.subTest(config=config, installed=installed), self.assertRaises(SystemExit):
                prepare_vocab.check_wordfreq(config, installed)

    def test_a_language_or_the_noise_without_words_stops_the_run(self):
        self.assertEqual(prepare_vocab.require_words('en-US', [('the', 0.05)]), [('the', 0.05)])
        with self.assertRaises(SystemExit):
            prepare_vocab.require_words('noise', [])

    def test_languages_json_names_the_exact_inputs(self):
        self.assertRegex(CONFIG['wordfreq'], r'^\d+\.\d+\.\d+$')
        for entry in CONFIG['languages']:
            self.assertRegex(entry['layout'], r'^[0-9A-F]{8}$', entry['id'])
            if entry['frequency'].startswith('file:'):
                self.assertRegex(entry['sha256'], r'^[0-9a-f]{64}$', entry['id'])
                self.assertTrue(entry['source'], entry['id'])


class Notice(unittest.TestCase):
    def test_the_notice_names_every_source_of_the_receipt(self):
        text = notice.render(config_receipt())
        self.assertIn(f"wordfreq {CONFIG['wordfreq']} by Robyn Speer", text)
        for entry in CONFIG['languages']:
            if 'sha256' in entry:
                self.assertIn(entry['sha256'], text)
        self.assertIn('German, French and Spanish words from wordfreq', text)

    def test_the_shipped_notice_is_the_one_written_from_the_receipt(self):
        shipped = (TOOLS.parent / 'data/layout-model/NOTICE.md').read_text(encoding='utf-8')
        self.assertEqual(shipped, notice.render(config_receipt()))

    def test_subtlex_is_credited_for_the_wordfreq_languages_that_carry_it(self):
        text = notice.render(config_receipt())
        for title in ('SUBTLEX-US', 'SUBTLEX-UK', 'SUBTLEX-DE'):
            self.assertIn(title, text)
        self.assertIn('SUBTLEX is freely available data', text)
        self.assertNotIn('SUBTLEX-NL', text)
        receipt = config_receipt()
        receipt['languages'] = [entry for entry in receipt['languages'] if entry['id'] != 'en-US']
        receipt['noise'] = ['fr', 'es']
        self.assertNotIn('SUBTLEX', notice.render(receipt))


@unittest.skipUnless(os.name == 'posix' and shutil.which('bash') and shutil.which('awk'), 'needs bash and awk')
class Pipeline(unittest.TestCase):
    def run_pipeline(self, failing_split):
        with tempfile.TemporaryDirectory() as scratch:
            scratch = Path(scratch)
            work = scratch / 'work'
            work.mkdir()
            trained = scratch / 'trained'
            python = scratch / 'python'
            # Stands in for every Python step; the case generator of `failing_split` fails.
            python.write_text(f'''#!/bin/sh
case "$1" in
  prepare_vocab.py) mkdir -p "$4" ;;
  notice.py) echo notice ;;
  generate_cases.py) [ "$6" = "{failing_split}" ] && exit 1; echo '{{"id": 0}}' > "$4" ;;
  train.py) [ "$2" = fit ] && touch "{trained}" ;;
esac
exit 0
''')
            python.chmod(0o755)
            env = dict(os.environ, WORK=str(work), PYTHON=str(python), OUTPUT=str(work / 'model.aklm'))
            run = subprocess.run(['bash', str(TOOLS / 'layout_model' / 'pipeline.sh')], env=env,
                                 capture_output=True, text=True, timeout=120)
            return run, trained.exists()

    def test_a_failed_case_generator_stops_the_pipeline_before_training(self):
        for split in ('train', 'test', 'uniform'):
            with self.subTest(split=split):
                run, trained = self.run_pipeline(split)
                self.assertNotEqual(run.returncode, 0, run.stdout + run.stderr)
                self.assertIn('a case generator failed', run.stderr)
                self.assertFalse(trained)

    def test_the_pipeline_trains_when_every_generator_succeeds(self):
        run, trained = self.run_pipeline('none')
        self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
        self.assertTrue(trained)


if __name__ == '__main__':
    unittest.main()
