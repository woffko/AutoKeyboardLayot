from pathlib import Path
import re
import unittest
import json
import hashlib
from audit_inno_translations import parse, parameters


class InstallerMessageTests(unittest.TestCase):
    def test_vendored_stock_catalogs_match_pinned_schema_and_hash(self):
        directory = Path(__file__).resolve().parents[1] / 'installer/vendor/inno'
        sources = json.loads((directory / 'SOURCES.json').read_text(encoding='utf-8'))
        reference = json.loads((directory / 'reference-6.7.3.json').read_text(encoding='utf-8'))
        self.assertEqual(sources['reference_version'], reference['version'])
        for record in sources['files']:
            with self.subTest(file=record['file']):
                raw = (directory / record['file']).read_bytes()
                self.assertEqual(hashlib.sha256(raw).hexdigest(), record['sha256'])
                sections = parse(raw.decode('utf-8-sig'))
                messages = sections['messages']
                if record['file'] == 'Urdu.isl':
                    self.assertEqual(sections['langoptions']['RightToLeft'], 'yes')
                    self.assertEqual(sections['langoptions']['LanguageID'], '$0420')
                if record['file'] == 'Estonian.isl':
                    self.assertEqual(sections['langoptions']['LanguageID'], '$0425')
                    additions = json.loads((directory / 'estonian-additions.json').read_text(encoding='utf-8'))
                    self.assertEqual(len(additions), 66)
                    for key, value in additions.items():
                        self.assertEqual(messages[key], value)
                if record['file'] == 'Hindi.isl':
                    self.assertEqual(sections['langoptions']['LanguageID'], '$0439')
                    updates = json.loads((directory / 'hindi-updates.json').read_text(encoding='utf-8'))
                    self.assertEqual(len(updates), 72)
                    for key, value in updates.items():
                        self.assertEqual(messages[key], value)
                if record['file'] == 'Bengali.isl':
                    self.assertEqual(sections['langoptions']['LanguageID'], '$0445')
                    updates = json.loads((directory / 'bengali-updates.json').read_text(encoding='utf-8'))
                    self.assertEqual(len(updates), 66)
                    for key, value in updates.items():
                        self.assertEqual(messages[key], value)
                    self.assertEqual(parameters(messages['SetupAppRunningError'])['%1'], 2)
                self.assertFalse(messages.keys() - reference['messages'].keys())
                for key, contract in reference['messages'].items():
                    if contract['required']:
                        self.assertTrue(messages.get(key), key)
                        self.assertEqual(set(parameters(messages[key])), set(contract['parameters']), key)

    def test_all_owned_messages_have_unique_fallbacks(self):
        root = Path(__file__).resolve().parents[1]
        source = (root / 'installer/AutoKeyboardLayot.iss').read_text(encoding='utf-8')
        self.assertIn('#include "package-pages.iss"', source)
        source += '\n' + (root / 'installer/package-pages.iss').read_text(encoding='utf-8')
        messages = (root / 'installer/messages/en.isl').read_text(encoding='utf-8')
        pairs = [line.split('=', 1) for line in messages.splitlines() if line.startswith('Ak')]
        catalog = dict(pairs)
        self.assertEqual(len(pairs), len(catalog))
        used = set(re.findall(r"CustomMessage\('(Ak\w+)'\)", source))
        used.update(re.findall(r'\{cm:(Ak\w+)\}', source))
        self.assertEqual(used, set(catalog))
        self.assertTrue(all(catalog.values()))
        self.assertEqual({key for key, text in pairs if '%1' in text}, {'AkCloseBusy', 'AkProfileReadFailed', 'AkPackageTotal', 'AkPackageReview', 'AkPackageRow'})
        self.assertNotRegex(source, r"Result\s*:=\s*'[^']+'")

    def test_locale_keys_and_placeholders_match_english_fallback(self):
        root = Path(__file__).resolve().parents[1] / 'installer'
        def catalog(name):
            pairs = [line.split('=', 1) for line in (root / 'messages' / name).read_text(encoding='utf-8').splitlines() if line.startswith('Ak')]
            result = dict(pairs)
            self.assertEqual(len(pairs), len(result), name)
            return result
        english = catalog('en.isl')
        source = (root / 'AutoKeyboardLayot.iss').read_text(encoding='utf-8')
        self.assertIn('#include "package-pages.iss"', source)
        source += '\n' + (root / 'package-pages.iss').read_text(encoding='utf-8')
        files = sorted((root / 'messages').glob('*.isl'))
        self.assertEqual({file.stem for file in files}, {'en', 'ru', 'de', 'es', 'fr', 'pt-BR', 'ja', 'ar', 'zh-CN', 'id', 'ur', 'et', 'hi', 'bn'})
        for file in files:
            with self.subTest(locale=file.stem):
                translated = catalog(file.name)
                self.assertEqual(set(english), set(translated))
                self.assertTrue(all(translated.values()))
                for key in english:
                    self.assertEqual(set(parameters(english[key])), set(parameters(translated[key])))
                if file.stem != 'en':
                    self.assertIn(',messages\\en.isl,messages\\' + file.name + '"', source)
        self.assertIn('LanguageDetectionMethod=uilanguage', source)
        self.assertIn('UsePreviousLanguage=no', source)


if __name__ == '__main__':
    unittest.main()
